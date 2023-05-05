use std::fmt;
use std::io;
use std::sync::PoisonError;

use tokio::sync::mpsc::error::SendError;

/// Error enum for this crate
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O Error
    IO(io::Error),

    /// Nom decode error
    Decode(nom::Err<nom::error::VerboseError<Vec<u8>>>),

    /// Channel closed
    ChannelClosed,

    /// Lock poison error,
    LockPoison,
}

/// Result type for this crate
pub type Result<T> = ::std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IO(ref e) => fmt::Display::fmt(e, f),
            Self::Decode(ref de) => fmt::Display::fmt(de, f),
            Self::ChannelClosed => write!(f, "Tried to write into a closed channel"),
            Self::LockPoison => write!(f, "Lock poison"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::IO(e)
    }
}

impl<T> From<SendError<T>> for Error {
    fn from(_: SendError<T>) -> Self {
        tracing::debug!("Transformed SendError into ChannelClosed");
        Self::ChannelClosed
    }
}

impl<T> From<PoisonError<T>> for Error {
    fn from(_: PoisonError<T>) -> Self {
        Self::LockPoison
    }
}

pub fn nom_to_owned(
    e: nom::Err<nom::error::VerboseError<&[u8]>>,
) -> nom::Err<nom::error::VerboseError<Vec<u8>>> {
    match e {
        nom::Err::Incomplete(i) => nom::Err::Incomplete(i),
        nom::Err::Error(mut e) => nom::Err::Error(nom::error::VerboseError {
            errors: e.errors.drain(..).map(|(i, e)| (i.to_owned(), e)).collect(),
        }),
        nom::Err::Failure(mut e) => nom::Err::Failure(nom::error::VerboseError {
            errors: e.errors.drain(..).map(|(i, e)| (i.to_owned(), e)).collect(),
        }),
    }
}

impl<'i> From<nom::Err<nom::error::VerboseError<&'i [u8]>>> for Error {
    fn from(e: nom::Err<nom::error::VerboseError<&'i [u8]>>) -> Self {
        let owned = nom_to_owned(e);
        Self::Decode(owned)
    }
}
