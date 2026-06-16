use crate::internal_prelude::*;
use clap::ValueEnum;
use serde::{Deserialize, Deserializer};
use std::str::FromStr;
use std::time::Duration;
use tokio_process_tools::UnixGracefulPhase;

/// Signal used on unix systems to gracefully shut down the user application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UnixSignal {
    #[value(name = "SIGINT")]
    Sigint,
    #[value(name = "SIGTERM")]
    Sigterm,
}

impl UnixSignal {
    pub fn to_graceful_shutdown_phase(self, timeout: Duration) -> UnixGracefulPhase {
        match self {
            UnixSignal::Sigint => UnixGracefulPhase::interrupt(timeout),
            UnixSignal::Sigterm => UnixGracefulPhase::terminate(timeout),
        }
    }
}

impl FromStr for UnixSignal {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("SIGINT") {
            Ok(UnixSignal::Sigint)
        } else if trimmed.eq_ignore_ascii_case("SIGTERM") {
            Ok(UnixSignal::Sigterm)
        } else {
            Err(eyre!(
                "Invalid UnixSignal `{trimmed}`. Expected one of: `SIGINT`, `SIGTERM`."
            ))
        }
    }
}

impl<'de> Deserialize<'de> for UnixSignal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <&str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod from_str {
        use super::*;

        #[test]
        fn accepts_case_insensitive() {
            assert_eq!("sigint".parse::<UnixSignal>().unwrap(), UnixSignal::Sigint);
            assert_eq!("SigInt".parse::<UnixSignal>().unwrap(), UnixSignal::Sigint);
            assert_eq!("SIGINT".parse::<UnixSignal>().unwrap(), UnixSignal::Sigint);
        }

        #[test]
        fn trims_whitespace() {
            assert_eq!(
                "  SIGINT\n".parse::<UnixSignal>().unwrap(),
                UnixSignal::Sigint
            );
        }

        #[test]
        fn rejects_unknown_signal() {
            assert_eq!(
                "SIGKILL".parse::<UnixSignal>().unwrap_err().to_string(),
                "Invalid UnixSignal `SIGKILL`. Expected one of: `SIGINT`, `SIGTERM`."
            );
        }
    }

    mod deserialize {
        use super::*;

        #[test]
        fn case_insensitive() {
            let s: UnixSignal = serde_json::from_str(r#""SIGTERM""#).unwrap();
            assert_eq!(s, UnixSignal::Sigterm);
            let s: UnixSignal = serde_json::from_str(r#""sigterm""#).unwrap();
            assert_eq!(s, UnixSignal::Sigterm);
        }
    }
}
