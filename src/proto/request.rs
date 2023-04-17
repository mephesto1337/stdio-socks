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

/// Requests that can be sent
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request<C = EmptyCustom> {
    /// Request a channel opening
    New {
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
    },
    /// Close the following channel
    Close { channel_id: ChannelId },
}

const REQUEST_TYPE_NEW: u8 = 1;
const REQUEST_TYPE_CLOSE: u8 = 2;

impl<C> Wire for Request<C>
where
    C: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::New {
                channel_id,
                ref endpoint,
            } => {
                buffer.push(REQUEST_TYPE_NEW);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                endpoint.encode_into(buffer);
            }
            Self::Close { channel_id } => {
                buffer.push(REQUEST_TYPE_CLOSE);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "Request",
            alt((
                preceded(
                    verify(be_u8, |b| *b == REQUEST_TYPE_NEW),
                    map(
                        tuple((be_u64, Endpoint::decode)),
                        |(channel_id, endpoint)| Self::New {
                            channel_id,
                            endpoint,
                        },
                    ),
                ),
                preceded(
                    verify(be_u8, |b| *b == REQUEST_TYPE_CLOSE),
                    map(be_u64, |channel_id| Self::Close { channel_id }),
                ),
            )),
        )(buffer)
    }
}

impl<C> fmt::Display for Request<C>
where
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::New {
                channel_id,
                ref endpoint,
            } => write!(
                f,
                "Request::New {{ channel_id: {}, endpoint: {} }}",
                channel_id, endpoint
            ),
            Self::Close { channel_id } => {
                write!(f, "Request::Close {{ channel_id: {} }}", channel_id)
            }
        }
    }
}
