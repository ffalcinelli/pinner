use thiserror::Error;

/// Custom error type for Pinner operations.
#[derive(Error, Debug)]
pub enum PinnerError {
    /// IO-related errors (file reading, writing, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Errors returned by the GitHub API or HTTP client.
    #[error("API error: {0}")]
    Api(String),
    /// Errors during YAML parsing (tree-sitter).
    #[error("Parse error: {0}")]
    Parse(String),
    /// Specified workflow path not found.
    #[error("Path not found: {0}")]
    PathNotFound(String),
    /// Unpinned dependencies found during verification.
    #[error("Verification failed: {0}")]
    VerificationFailed(String),
    /// Errors from the `ignore` crate during directory walking.
    #[error("Ignore error: {0}")]
    Ignore(#[from] ignore::Error),
    /// Config file errors
    #[error("Config error: {0}")]
    Config(String),
    /// API rate limit errors
    #[error("Rate limit error: {0}")]
    RateLimit(String),
    /// Authentication errors (invalid tokens, etc.)
    #[error("Authentication error: {0}")]
    Authentication(String),
    /// OCI Registry errors
    #[error("Registry error: {0}")]
    Registry(String),
    /// HTTP errors from reqwest
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Errors from reqwest-middleware
    #[error("Middleware error: {0}")]
    Middleware(#[from] reqwest_middleware::Error),
}

impl PinnerError {
    /// Returns true if the error is a PathNotFound error.
    pub fn is_path_not_found(&self) -> bool {
        matches!(self, PinnerError::PathNotFound(_))
    }

    /// Returns true if the error should stop the entire process (e.g., rate limits).
    pub fn is_fatal(&self) -> bool {
        match self {
            PinnerError::RateLimit(_) => true,
            PinnerError::Authentication(_) => true,
            PinnerError::Config(_) => true,
            PinnerError::Io(_) => true,
            PinnerError::Parse(_) => true,
            PinnerError::PathNotFound(_) => true,
            PinnerError::VerificationFailed(_) => true,
            PinnerError::Ignore(_) => true,
            // Generic API errors (like 404) or Registry errors for a single image are not fatal
            PinnerError::Api(_) => false,
            PinnerError::Registry(_) => false,
            PinnerError::Http(_) => false,
            PinnerError::Middleware(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_error_display() {
        let err = PinnerError::Api("failed".to_string());
        assert_eq!(format!("{}", err), "API error: failed");
        assert!(!err.is_path_not_found());

        let err = PinnerError::Parse("yaml".to_string());
        assert_eq!(format!("{}", err), "Parse error: yaml");

        let err = PinnerError::PathNotFound("path/to/wf".to_string());
        assert_eq!(format!("{}", err), "Path not found: path/to/wf");
        assert!(err.is_path_not_found());

        let err = PinnerError::Config("invalid".to_string());
        assert_eq!(format!("{}", err), "Config error: invalid");

        let err = PinnerError::VerificationFailed("unpinned".to_string());
        assert_eq!(format!("{}", err), "Verification failed: unpinned");

        let err = PinnerError::RateLimit("exceeded".to_string());
        assert_eq!(format!("{}", err), "Rate limit error: exceeded");
    }

    #[test]
    fn test_error_is_fatal() {
        assert!(PinnerError::RateLimit("".into()).is_fatal());
        assert!(PinnerError::Config("".into()).is_fatal());
        assert!(PinnerError::Io(io::Error::other("")).is_fatal());
        assert!(PinnerError::Parse("".into()).is_fatal());
        assert!(PinnerError::PathNotFound("".into()).is_fatal());
        assert!(PinnerError::VerificationFailed("".into()).is_fatal());
        assert!(PinnerError::Ignore(ignore::Error::Io(io::Error::other(""))).is_fatal());
        assert!(!PinnerError::Api("".into()).is_fatal());
    }

    #[test]
    fn test_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "not found");
        let err = PinnerError::from(io_err);
        assert!(matches!(err, PinnerError::Io(_)));
        assert!(format!("{}", err).contains("IO error: not found"));
    }

    #[test]
    fn test_error_from_ignore() {
        let io_err = io::Error::other("ignore err");
        let err = PinnerError::Ignore(ignore::Error::Io(io_err));
        assert!(format!("{}", err).contains("Ignore error"));
    }
}
