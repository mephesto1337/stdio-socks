use std::fmt;

use crate::ChannelId;

use super::{
    endpoint::{EmptyCustom, Endpoint},
    Wire,
};

use nom::{
    branch::alt,
    combinator::{map, verify},
    error::context,
    number::streaming::{be_u64, be_u8},
    sequence::{preceded, tuple},
};

/// Responses to requests
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response<C = EmptyCustom> {
    /// Channel `channel_id` was successfully opened
    New {
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
    },
    /// Channel `channel_id` was successfully closed
    Close { channel_id: ChannelId },
    /// An error occured with channel `channel_id`
    Error {
        channel_id: ChannelId,
        message: String,
    },
}

const RESPONSE_TYPE_NEW: u8 = 1;
const RESPONSE_TYPE_CLOSE: u8 = 2;
const RESPONSE_TYPE_ERROR: u8 = 3;

impl<C> Wire for Response<C>
where
    C: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::New {
                channel_id,
                endpoint,
            } => {
                buffer.push(RESPONSE_TYPE_NEW);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                endpoint.encode_into(buffer);
            }
            Self::Close { channel_id } => {
                buffer.push(RESPONSE_TYPE_CLOSE);
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
            RESPONSE_TYPE_NEW => context(
                "Response::New",
                map(
                    tuple((be_u64, Endpoint::decode)),
                    |(channel_id, endpoint)| Self::New {
                        channel_id,
                        endpoint,
                    },
                ),
            )(rest),

            RESPONSE_TYPE_CLOSE => context(
                "Response::Close",
                map(be_u64, |channel_id| Self::Close { channel_id }),
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

impl<C> fmt::Display for Response<C>
where
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::New {
                channel_id,
                endpoint,
            } => write!(
                f,
                "Response::New {{ channel_id: {}, endpoint: {} }}",
                channel_id, endpoint
            ),
            Self::Close { channel_id } => {
                write!(f, "Response::Close {{ channel_id: {} }}", channel_id)
            }
            Self::Error {
                channel_id,
                ref message,
            } => write!(
                f,
                "Response::Error {{ channel_id: {}, message: {} }}",
                channel_id, message
            ),
        }
    }
}
