use std::fmt;

/// Structured error covering every failure mode an LLM provider can surface.
///
/// Variants are intentionally coarse-grained so callers can pattern-match on
/// the category (auth, rate-limit, network, etc.) without coupling to a
/// specific provider's error representation.
#[derive(Debug)]
pub enum LlmError {
    /// No credentials have been configured for the selected provider.
    ProviderNotConfigured,
    /// The provider configuration is syntactically or semantically invalid.
    InvalidConfig(String),
    /// The API key was rejected by the provider.
    Unauthorized,
    /// The provider returned HTTP 429; `retry_after` is the suggested back-off.
    RateLimited { retry_after: Option<u64> },
    /// A non-recoverable HTTP error from the provider.
    ServerError { status: u16, body: String },
    /// Transport-level failure (DNS, TLS, timeout, etc.).
    NetworkError(String),
    /// The provider returned a response that could not be parsed.
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
