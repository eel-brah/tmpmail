use thiserror::Error;

/// Every failure the CLI can report. Each variant maps to a stable exit code
/// so scripts can tell "no mail yet" apart from "provider is down".
#[derive(Debug, Error)]
pub enum Error {
    #[error("{provider}: unreachable ({detail})")]
    Unreachable { provider: String, detail: String },

    #[error("{provider}: rate limited, try again later")]
    RateLimited { provider: String },

    #[error("{provider}: authentication failed ({detail})")]
    Auth { provider: String, detail: String },

    #[error("{provider}: unexpected response ({detail})")]
    BadResponse { provider: String, detail: String },

    #[error("{0}")]
    NotFound(String),

    #[error("{provider}: {what} is not supported by this provider")]
    Unsupported { provider: String, what: String },

    #[error("{0}")]
    AddressTaken(String),

    #[error("inbox {0} has expired")]
    Expired(String),

    #[error("no inbox yet, create one with `tmpmail new`")]
    NoInbox,

    #[error("timed out after {0}s without a matching message")]
    Timeout(u64),

    #[error("{0}")]
    Usage(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Timeout(_) => 2,
            Error::Unreachable { .. }
            | Error::RateLimited { .. }
            | Error::BadResponse { .. }
            | Error::Auth { .. } => 3,
            Error::NotFound(_) | Error::NoInbox | Error::Expired(_) => 4,
            Error::Usage(_) | Error::Unsupported { .. } | Error::AddressTaken(_) => 64,
            Error::Other(_) => 1,
        }
    }

    /// Errors that justify trying the next provider during `new`.
    pub fn is_provider_failure(&self) -> bool {
        matches!(
            self,
            Error::Unreachable { .. }
                | Error::RateLimited { .. }
                | Error::BadResponse { .. }
                | Error::Auth { .. }
        )
    }

    /// Short machine-readable kind, used in `--json` error output.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Unreachable { .. } => "unreachable",
            Error::RateLimited { .. } => "rate_limited",
            Error::Auth { .. } => "auth",
            Error::BadResponse { .. } => "bad_response",
            Error::NotFound(_) => "not_found",
            Error::Unsupported { .. } => "unsupported",
            Error::AddressTaken(_) => "address_taken",
            Error::Expired(_) => "expired",
            Error::NoInbox => "no_inbox",
            Error::Timeout(_) => "timeout",
            Error::Usage(_) => "usage",
            Error::Other(_) => "error",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn unsupported(provider: &str, what: &str) -> Error {
    Error::Unsupported {
        provider: provider.to_string(),
        what: what.to_string(),
    }
}
