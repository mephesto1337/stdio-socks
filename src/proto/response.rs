use std::fmt;

use crate::ChannelId;

use super::Wire;

use nom::{
    combinator::map,
    error::context,
    number::streaming::{be_u64, be_u8},
    sequence::tuple,
};

/// Responses to requests
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Channel `channel_id` was successfully opened
    Open { channel_id: ChannelId },
    /// An error occured with channel `channel_id`
    Error {
        channel_id: ChannelId,
        message: String,
    },
}

impl Response {
    pub fn get_channel_id(&self) -> ChannelId {
        match self {
            Response::Open { channel_id } => *channel_id,
            Response::Error { channel_id, .. } => *channel_id,
        }
    }
}

const RESPONSE_TYPE_OPEN: u8 = 1;
const RESPONSE_TYPE_ERROR: u8 = 2;

impl Wire for Response {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::Open { channel_id } => {
                buffer.push(RESPONSE_TYPE_OPEN);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
            }
            Self::Error {
                channel_id,
                ref message,
            } => {
                buffer.push(RESPONSE_TYPE_ERROR);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                message.encode_into(buffer)
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, response_type) = be_u8(buffer)?;

        match response_type {
            RESPONSE_TYPE_OPEN => context(
                "Response::Open",
                map(be_u64, |channel_id| Self::Open { channel_id }),
            )(rest),

            RESPONSE_TYPE_ERROR => context(
                "Response::Error",
                map(tuple((be_u64, String::decode)), |(channel_id, message)| {
                    Self::Error {
                        channel_id,
                        message,
                    }
                }),
            )(rest),

            _ => Err(nom::Err::Failure(E::add_context(
                buffer,
                "Invalid response type",
                nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
            ))),
        }
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { channel_id } => write!(f, "Response::Open {{ channel_id: {channel_id} }}"),
            Self::Error {
                channel_id,
                ref message,
            } => write!(
                f,
                "Response::Error {{ channel_id: {channel_id}, message: {message} }}",
            ),
        }
    }
}
