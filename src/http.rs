//! Shared plumbing for the providers that poll an endpoint rather than reading
//! a local file.

pub enum FetchError {
    RateLimited,
    Unauthorized,
    Offline,
    Timeout,
    Other(anyhow::Error),
}

/// These strings are rendered in a 208pt-wide dock, so they stay terse.
/// `reqwest`'s own connect error runs several times the card width.
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => write!(f, "rate limited"),
            Self::Unauthorized => write!(f, "re-auth needed"),
            Self::Offline => write!(f, "offline"),
            Self::Timeout => write!(f, "timed out"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

pub fn classify(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else if e.is_connect() || e.is_request() {
        FetchError::Offline
    } else {
        FetchError::Other(e.into())
    }
}

pub fn client() -> std::result::Result<reqwest::blocking::Client, FetchError> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Other(e.into()))
}
