use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZapcodeError {
    #[error("parse error: {0}")]
    ParseError(String),

    #[error("unsupported syntax at {span}: {description}")]
    UnsupportedSyntax { span: String, description: String },

    #[error("compile error: {0}")]
    CompileError(String),

    #[error("runtime error: {0}")]
    RuntimeError(String),

    #[error("type error: {0}")]
    TypeError(String),

    #[error("range error: {0}")]
    RangeError(String),

    #[error("reference error: {0} is not defined")]
    ReferenceError(String),

    #[error("unknown external function: {0}")]
    UnknownExternalFunction(String),

    #[error("external function error: {0}")]
    ExternalError(String),

    #[error("memory limit exceeded: {0}")]
    MemoryLimitExceeded(String),

    #[error("execution time limit exceeded")]
    TimeLimitExceeded,

    #[error("stack overflow (depth {0})")]
    StackOverflow(usize),

    #[error("allocation limit exceeded")]
    AllocationLimitExceeded,

    #[error("snapshot error: {0}")]
    SnapshotError(String),

    #[error("sandbox violation: {0}")]
    SandboxViolation(String),
}

impl ZapcodeError {
    /// Append a source-location suffix (e.g. "\n    at 3:7\n    <code frame>")
    /// to the error's message. Only applies to variants whose payload is the
    /// *trailing* text of the rendered message, so the existing first line is
    /// preserved exactly. Variants with no payload or a mid-message payload
    /// (`ReferenceError` renders as "{0} is not defined") are returned unchanged.
    pub(crate) fn with_location_suffix(self, suffix: &str) -> Self {
        use ZapcodeError::*;
        match self {
            RuntimeError(s) => RuntimeError(format!("{s}{suffix}")),
            TypeError(s) => TypeError(format!("{s}{suffix}")),
            RangeError(s) => RangeError(format!("{s}{suffix}")),
            ExternalError(s) => ExternalError(format!("{s}{suffix}")),
            UnknownExternalFunction(s) => UnknownExternalFunction(format!("{s}{suffix}")),
            other => other,
        }
    }
}

pub type Result<T> = std::result::Result<T, ZapcodeError>;
