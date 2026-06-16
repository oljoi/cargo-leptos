use crate::{
    config::{Project, ShutdownPolicy},
    ext::{append_str_to_filename, determine_pdb_filename, fs, Paint},
    internal_prelude::*,
    logger::GRAY,
    signal::{Interrupt, ReloadSignal, ServerRestart},
};
use camino::Utf8PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWrite;
use tokio::{join, process::Command, select, task::JoinHandle};
use tokio_process_tools::{
    AutoName, Consumable, Consumer, GracefulShutdown, LineOverflowBehavior, LineParsingOptions,
    Next, NumBytesExt, ParseLines, Process, ProcessHandle, ReliableWithBackpressure, ReplayEnabled,
    SingleSubscriberOutputStream, UnixGracefulShutdown, WindowsGracefulShutdown,
    DEFAULT_MAX_BUFFERED_CHUNKS, DEFAULT_MAX_LINE_LENGTH, DEFAULT_READ_CHUNK_SIZE,
};

const INSPECTOR_CANCEL_TIMEOUT: Duration = Duration::from_secs(1);

type ServerProcessHandle =
    ProcessHandle<SingleSubscriberOutputStream<ReliableWithBackpressure, ReplayEnabled>>;

pub async fn spawn(proj: &Arc<Project>) -> JoinHandle<Result<()>> {
    let mut int = Interrupt::subscribe_shutdown();
    let proj = proj.clone();
    let mut change = ServerRestart::subscribe();
    tokio::spawn(async move {
        let mut server = ServerProcess::start_new(&proj).await?;
        loop {
            select! {
                res = change.recv() => {
                    if let Ok(()) = res {
                          server.restart().await?;
                          ReloadSignal::send_full();
                    }
                },
                _ = int.recv() => {
                    info!("Serve received interrupt signal");
                    server.terminate().await;
                    return Ok(())
                },
            }
        }
    })
}

pub async fn spawn_oneshot(proj: &Arc<Project>) -> JoinHandle<Result<()>> {
    let mut int = Interrupt::subscribe_shutdown();
    let proj = proj.clone();
    tokio::spawn(async move {
        let mut server = ServerProcess::start_new(&proj).await?;
        select! {
          _ = server.wait() => {},
          _ = int.recv() => {
                server.terminate().await;
          },
        }
        Ok(())
    })
}

struct ServerProcess {
    process: Option<(ServerProcessHandle, Consumer<()>, Consumer<()>)>,
    envs: Vec<(&'static str, String)>,
    binary: Utf8PathBuf,
    bin_args: Option<Vec<String>>,
    shutdown_policy: ShutdownPolicy,
}

impl ServerProcess {
    fn new(proj: &Project) -> Self {
        Self {
            process: None,
            envs: proj.to_envs(false),
            binary: proj.bin.exe_file.clone(),
            bin_args: proj.bin.bin_args.clone(),
            shutdown_policy: proj.shutdown_policy,
        }
    }

    async fn start_new(proj: &Project) -> Result<Self> {
        let mut me = Self::new(proj);
        me.start().await?;
        Ok(me)
    }

    async fn terminate(&mut self) {
        let Some((mut handle, stdout_inspector, stderr_inspector)) = self.process.take() else {
            return;
        };

        let result = if self.shutdown_policy.graceful {
            let timeout = self.shutdown_policy.timeout;
            handle
                .terminate(
                    GracefulShutdown::builder()
                        .unix(UnixGracefulShutdown::from_phases([self
                            .shutdown_policy
                            .unix_signal
                            .to_graceful_shutdown_phase(timeout)]))
                        .windows(WindowsGracefulShutdown::new(timeout))
                        .build(),
                )
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        } else {
            handle.kill().await.map_err(anyhow::Error::from)
        };

        if let Err(e) = result {
            error!("Serve error terminating server process: {e}");
        } else {
            trace!("Serve stopped");
        }

        let (stdout_err, stderr_err) = join!(
            stdout_inspector.cancel(INSPECTOR_CANCEL_TIMEOUT),
            stderr_inspector.cancel(INSPECTOR_CANCEL_TIMEOUT)
        );
        if let Err(err) = stdout_err {
            error!("Serve error aborting stdout inspector: {err}");
        };
        if let Err(err) = stderr_err {
            error!("Serve error aborting stderr inspector: {err}");
        };
    }

    async fn restart(&mut self) -> Result<()> {
        self.terminate().await;
        self.start().await?;
        trace!("Serve restarted");
        Ok(())
    }

    async fn wait(&mut self) -> Result<()> {
        if let Some((process, _, _)) = self.process.as_mut() {
            if let Err(e) = process.wait_for_completion(Duration::MAX).await {
                error!("Serve error while waiting for server process to exit: {e}");
            } else {
                trace!("Serve process exited");
            }
        }
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let bin = &self.binary;
        let process = if bin.exists() {
            // windows doesn't like to overwrite a running binary, so we copy it to a new name
            let bin_path = if cfg!(target_os = "windows") {
                // solution to allow cargo to overwrite a running binary on some platforms:
                //   copy cargo's output bin to [filename]_leptos and then run it
                let new_bin_path = append_str_to_filename(bin, "_leptos")?;
                debug!(
                    "Copying server binary {} to {}",
                    GRAY.paint(bin.as_str()),
                    GRAY.paint(new_bin_path.as_str())
                );
                fs::copy(bin, &new_bin_path).await?;
                // also copy the .pdb file if it exists to allow debugging to attach
                if let Some(pdb) = determine_pdb_filename(bin) {
                    let new_pdb_path = append_str_to_filename(&pdb, "_leptos")?;
                    debug!(
                        "Copying server binary debug info {} to {}",
                        GRAY.paint(pdb.as_str()),
                        GRAY.paint(new_pdb_path.as_str())
                    );
                    fs::copy(&pdb, &new_pdb_path).await?;
                }
                new_bin_path
            } else {
                bin.clone()
            };

            let bin_args = match &self.bin_args {
                Some(bin_args) => bin_args.as_slice(),
                None => &[],
            };

            debug!("Serve running {}", GRAY.paint(bin_path.as_str()));
            let mut cmd = Command::new(bin_path);
            cmd.envs(self.envs.clone());
            cmd.args(bin_args);

            let handle = Process::new(cmd)
                .name(AutoName::program_only())
                .stdout_and_stderr(|stream| {
                    stream
                        .single_subscriber()
                        .reliable_with_backpressure()
                        // Replay buffer only needs to bridge the gap between process spawn and the
                        // first consumer attaching below. We call `seal_replay()` immediately
                        // after, so 256 KB is plenty.
                        .replay_last_bytes(256.kilobytes())
                        .read_chunk_size(DEFAULT_READ_CHUNK_SIZE)
                        .max_buffered_chunks(DEFAULT_MAX_BUFFERED_CHUNKS)
                })
                .spawn()?;

            async fn write_to<W: AsyncWrite + Unpin>(
                mut to: W,
                data: &str,
            ) -> tokio::io::Result<()> {
                use tokio::io::AsyncWriteExt;
                to.write_all(data.as_bytes()).await?;
                to.write_all(b"\n").await?;
                to.flush().await?;
                Ok(())
            }

            // Let's forward captured stdout/stderr lines to the output of our process. We do this
            // asynchronously using the tokio::io::std{out|err}() handles, as writing to
            // stdout/stderr directly using print!() could result in unhandled "failed printing to
            // stdout: Resource temporarily unavailable" errors should the cargo-leptos output be
            // consumed too slowly. This can happen because tokio puts the stdio fds into
            // non-blocking mode (once touched) and std print! has no support for that, they just
            // panic when an EAGAIN error is observed. Tokio's stdio handles instead asynchronously
            // wait internally, handling the slow drainage and preventing a blocked runtime.
            let line_parsing_options = LineParsingOptions::builder()
                .max_line_length(DEFAULT_MAX_LINE_LENGTH)
                .overflow_behavior(LineOverflowBehavior::DropAdditionalData)
                .buffer_compaction_threshold(None)
                .build();
            let stdout_inspector = handle.stdout().consume_async(ParseLines::inspect_async(
                line_parsing_options,
                |line| {
                    let line = line.to_string();
                    async move {
                        if let Err(err) = write_to(tokio::io::stdout(), &line).await {
                            error!("Could not forward server process output to stdout: {err}");
                        }
                        Next::Continue
                    }
                },
            ))?;
            let stderr_inspector = handle.stderr().consume_async(ParseLines::inspect_async(
                line_parsing_options,
                |line| {
                    let line = line.to_string();
                    async move {
                        if let Err(err) = write_to(tokio::io::stderr(), &line).await {
                            error!("Could not forward server process output to stderr: {err}");
                        }
                        Next::Continue
                    }
                },
            ))?;

            // Inspectors are attached above so they receive the replay buffer. We don't attach any
            // late subscribers after, so we can safely inform the output stream here to not
            // collect replay information anymore. We were only interested in filling the gap
            // between process start and first consumer attachment.
            handle.stdout().seal_replay();
            handle.stderr().seal_replay();

            let port = self
                .envs
                .iter()
                .find_map(|(k, v)| {
                    if k == &"LEPTOS_SITE_ADDR" {
                        Some(v.to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let suffix = if self.shutdown_policy.graceful {
                " (with graceful shutdown)"
            } else {
                ""
            };
            info!("Serving at http://{port}{suffix}");
            Some((handle, stdout_inspector, stderr_inspector))
        } else {
            debug!("Serve no exe found {}", GRAY.paint(bin.as_str()));
            None
        };
        self.process = process;
        Ok(())
    }
}
