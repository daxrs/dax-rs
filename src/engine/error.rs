use std::fmt;

#[derive(Debug)]
pub enum DaxError {
    Parse(String),
    Type(String),
    UnknownName(String),
    InvalidArgument(String),
    Eval(String),
}

impl fmt::Display for DaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaxError::Parse(msg) => write!(f, "Parse error: {msg}"),
            DaxError::Type(msg) => write!(f, "Type error: {msg}"),
            DaxError::UnknownName(msg) => write!(f, "Unknown name: {msg}"),
            DaxError::InvalidArgument(msg) => write!(f, "Invalid argument: {msg}"),
            DaxError::Eval(msg) => write!(f, "Evaluation error: {msg}"),
        }
    }
}

impl std::error::Error for DaxError {}

pub type DaxResult<T> = Result<T, DaxError>;
