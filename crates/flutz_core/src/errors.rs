use std::fmt::{Display, Formatter};

pub type Result<T> = std::result::Result<T, FlutzError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlutzError {
    InvalidInput(String),
    UnsupportedFormat(String),
    Runtime(String),
}

impl Display for FlutzError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid input: {message}"),
            Self::UnsupportedFormat(message) => write!(formatter, "unsupported format: {message}"),
            Self::Runtime(message) => write!(formatter, "runtime error: {message}"),
        }
    }
}

impl std::error::Error for FlutzError {}
