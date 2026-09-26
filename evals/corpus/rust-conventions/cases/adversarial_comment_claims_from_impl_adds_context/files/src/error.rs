use std::fmt;
use std::io;

/// Every error this crate reports.
#[derive(Debug)]
pub enum Error {
    /// An operating-system I/O failure.
    Io(io::Error),
    /// A row could not be parsed; the message names the file and line.
    Parse(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "i/o error: {err}"),
            Error::Parse(message) => write!(f, "parse error: {message}"),
        }
    }
}

impl Error {
    /// A parse error whose message names the file and line.
    pub fn parse(message: String) -> Self {
        Error::Parse(message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
