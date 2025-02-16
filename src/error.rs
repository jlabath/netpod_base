use bendy::decoding;
use bendy::encoding;
use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NetpodError {
    #[error("netpod io error: {0}")]
    IO(#[from] io::Error),
    #[error("{0}")]
    Message(String),
    #[error("bendy decoding error: {0}")]
    BendyDecoding(decoding::Error),
    #[error("bendy encoding error: {0}")]
    BendyEncoding(encoding::Error),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>), // Accepts any error
}

impl From<&str> for NetpodError {
    fn from(s: &str) -> Self {
        NetpodError::Message(s.to_string())
    }
}

impl From<String> for NetpodError {
    fn from(s: String) -> Self {
        NetpodError::Message(s)
    }
}

impl From<decoding::Error> for NetpodError {
    fn from(e: decoding::Error) -> Self {
        NetpodError::BendyDecoding(e)
    }
}

impl From<encoding::Error> for NetpodError {
    fn from(e: encoding::Error) -> Self {
        NetpodError::BendyEncoding(e)
    }
}

pub fn from_error<E>(err: E) -> NetpodError
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    NetpodError::Other(err.into())
}
