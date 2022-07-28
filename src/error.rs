use std::fmt;
use std::io;
use std::num::TryFromIntError;
use std::string::FromUtf8Error;
use std::sync::PoisonError;

use tokio::sync::mpsc::error::SendError;

/// Error enum for this crate
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O Error
    IO(io::Error),

    /// Nom decode error
    Decode(nom::Err<nom::error::VerboseError<Vec<u8>>>),

    /// Invalid port number
    InvalidPortNumber(u32),

    /// Channel closed
    ChannelClosed,

    /// Integer truncation
    IntegerTruncation(TryFromIntError),

    /// Invalid enum value
    InvalidEnumValue(u64, &'static str),

    /// Invalid UTF-8 Conversion
    UTF8(FromUtf8Error),

    /// Lock poison error,
    LockPoison,

    /// Address resolution error
    AddressResolution(String),
}

pub type Result<T> = ::std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IO(ref e) => fmt::Display::fmt(e, f),
            Self::Decode(ref de) => fmt::Display::fmt(de, f),
            Self::InvalidPortNumber(p) => write!(f, "Invalid port numnber {}", p),
            Self::ChannelClosed => write!(f, "Tried to write into a closed channel"),
            Self::IntegerTruncation(ref e) => write!(f, "Integer truncation: {}", e),
            Self::InvalidEnumValue(value, name) => {
                write!(f, "Found unexpected enum value {} for {}", value, name)
            }
            Self::UTF8(ref e) => write!(f, "UTF8 conversion error: {}", e),
            Self::LockPoison => write!(f, "Lock poison"),
            Self::AddressResolution(ref name) => write!(f, "Cannot resolve name {}", name),
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

impl From<TryFromIntError> for Error {
    fn from(e: TryFromIntError) -> Self {
        Self::IntegerTruncation(e)
    }
}

impl From<FromUtf8Error> for Error {
    fn from(e: FromUtf8Error) -> Self {
        Self::UTF8(e)
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
