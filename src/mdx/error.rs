#[derive(Debug)]
pub enum MdxError {
    Parse(String),
    Translate(String),
}

impl std::fmt::Display for MdxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MdxError::Parse(e) => write!(f, "MDX parse error: {e}"),
            MdxError::Translate(e) => write!(f, "MDX translate error: {e}"),
        }
    }
}

impl std::error::Error for MdxError {}
