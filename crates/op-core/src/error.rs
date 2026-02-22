use std::fmt;

/// Unified error type for the OpenPlanter system.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("Model error: {0}")]
    Model(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Patch error: {0}")]
    Patch(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("JSON error: {source}")]
    Json {
        #[from]
        source: serde_json::Error,
    },
}

pub type OpResult<T> = Result<T, OpError>;

impl OpError {
    pub fn model(msg: impl fmt::Display) -> Self {
        Self::Model(msg.to_string())
    }

    pub fn tool(msg: impl fmt::Display) -> Self {
        Self::Tool(msg.to_string())
    }

    pub fn session(msg: impl fmt::Display) -> Self {
        Self::Session(msg.to_string())
    }

    pub fn patch(msg: impl fmt::Display) -> Self {
        Self::Patch(msg.to_string())
    }

    pub fn config(msg: impl fmt::Display) -> Self {
        Self::Config(msg.to_string())
    }

    pub fn http(msg: impl fmt::Display) -> Self {
        Self::Http(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let e = OpError::model("test error");
        assert_eq!(e.to_string(), "Model error: test error");
    }

    #[test]
    fn test_error_variants() {
        let e = OpError::tool("bad tool");
        assert!(matches!(e, OpError::Tool(_)));
        let e = OpError::session("no session");
        assert!(matches!(e, OpError::Session(_)));
        let e = OpError::patch("bad patch");
        assert!(matches!(e, OpError::Patch(_)));
        let e = OpError::config("bad config");
        assert!(matches!(e, OpError::Config(_)));
        let e = OpError::http("timeout");
        assert!(matches!(e, OpError::Http(_)));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let op_err: OpError = io_err.into();
        assert!(matches!(op_err, OpError::Io { .. }));
    }
}
