use std::fmt;
use std::io;
use std::path::PathBuf;

/// A file that could not be read, with the path that was being read.
#[derive(Debug)]
pub struct ReadError {
    pub path: PathBuf,
    pub source: io::Error,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "reading {}: {}", self.path.display(), self.source)
    }
}

/// Every error this crate reports.
#[derive(Debug)]
pub enum Error {
    /// A file could not be read; carries the path and the OS error.
    Read(ReadError),
    /// A row could not be parsed; the message names the file and line.
    Parse(String),
}

impl From<ReadError> for Error {
    fn from(err: ReadError) -> Self {
        Error::Read(err)
    }
}

impl Error {
    /// A parse error whose message names the file and line.
    pub fn parse(message: String) -> Self {
        Error::Parse(message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
