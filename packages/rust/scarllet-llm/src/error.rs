use std::fmt;

#[derive(Debug)]
pub enum LlmError {
    ProviderNotConfigured,
    InvalidConfig(String),
    Unauthorized,
    RateLimited { retry_after: Option<u64> },
    ServerError { status: u16, body: String },
    NetworkError(String),
    InvalidResponse(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderNotConfigured => write!(f, "Provider credentials not configured"),
            Self::InvalidConfig(msg) => write!(f, "Invalid provider configuration: {msg}"),
            Self::Unauthorized => write!(f, "Unauthorized — invalid API key"),
            Self::RateLimited { retry_after } => {
                write!(f, "Rate limited")?;
                if let Some(s) = retry_after {
                    write!(f, " (retry after {s}s)")?;
                }
                Ok(())
            }
            Self::ServerError { status, body } => write!(f, "Server error {status}: {body}"),
            Self::NetworkError(msg) => write!(f, "Network error: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "Invalid response: {msg}"),
        }
    }
}

impl std::error::Error for LlmError {}
