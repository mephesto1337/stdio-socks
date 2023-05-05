use std::fmt;

use crate::ChannelId;

use super::{endpoint::RawCustom, response::Response, Endpoint, Wire};

use nom::{
    combinator::{map, rest},
    error::context,
    number::streaming::{be_u64, be_u8},
    sequence::tuple,
};

/// Messages that can be exchanged
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message<C = RawCustom> {
    /// A request to open a new channel
    RequestOpen {
        /// Channel ID wanted by client
        channel_id: ChannelId,

        /// endpoint to connect to (passed to [`crate::OpenStreamFn`]
        endpoint: Endpoint<C>,
    },
    /// A response message
    Response(Response),
    /// Data between 2 endpoints
    Data {
        /// Destination channel ID
        channel_id: ChannelId,

        /// Data to be passed
        data: Vec<u8>,
    },
    #[cfg(feature = "heartbeat")]
    /// Ping to keep stream open, ping request
    Ping(u64),
    #[cfg(feature = "heartbeat")]
    /// Ping to keep stream open, ping reply
    Pong(u64),
}
const MESSAGE_TYPE_REQUEST_OPEN: u8 = 1;
const MESSAGE_TYPE_RESPONSE: u8 = 2;
const MESSAGE_TYPE_DATA: u8 = 3;
#[cfg(feature = "heartbeat")]
const MESSAGE_TYPE_PING: u8 = 4;
#[cfg(feature = "heartbeat")]
const MESSAGE_TYPE_PONG: u8 = 5;

fn parse_message<'i, C, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Message<C>, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    C: Wire,
{
    let (input, message_type) = be_u8(buffer)?;

    match message_type {
        MESSAGE_TYPE_REQUEST_OPEN => map(
            tuple((be_u64, Endpoint::<C>::decode)),
            |(channel_id, endpoint)| Message::RequestOpen {
                channel_id,
                endpoint,
            },
        )(input),
        MESSAGE_TYPE_RESPONSE => map(Response::decode, Message::Response)(input),
        MESSAGE_TYPE_DATA => map(tuple((be_u64, rest)), |(channel_id, data): (_, &[u8])| {
            Message::Data {
                channel_id,
                data: data.to_vec(),
            }
        })(input),
        #[cfg(feature = "heartbeat")]
        MESSAGE_TYPE_PING => map(be_u64, Message::<C>::Ping)(input),
        #[cfg(feature = "heartbeat")]
        MESSAGE_TYPE_PONG => map(be_u64, Message::<C>::Pong)(input),
        _ => Err(nom::Err::Failure(E::add_context(
            buffer,
            "Invalid message type",
            nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
        ))),
    }
}

impl<C> Wire for Message<C>
where
    C: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::RequestOpen {
                channel_id,
                ref endpoint,
            } => {
                buffer.push(MESSAGE_TYPE_REQUEST_OPEN);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                endpoint.encode_into(buffer);
            }
            Self::Response(ref response) => {
                buffer.push(MESSAGE_TYPE_RESPONSE);
                response.encode_into(buffer);
            }
            Self::Data {
                channel_id,
                ref data,
            } => {
                buffer.push(MESSAGE_TYPE_DATA);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                buffer.extend_from_slice(data);
            }
            #[cfg(feature = "heartbeat")]
            Self::Ping(id) => {
                buffer.push(MESSAGE_TYPE_PING);
                buffer.extend_from_slice(&id.to_be_bytes()[..]);
            }
            #[cfg(feature = "heartbeat")]
            Self::Pong(id) => {
                buffer.push(MESSAGE_TYPE_PONG);
                buffer.extend_from_slice(&id.to_be_bytes()[..]);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context("Message", parse_message)(buffer)
    }
}

impl<C> fmt::Display for Message<C>
where
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestOpen {
                channel_id,
                endpoint,
            } => write!(
                f,
                "RequestOpen {{ channel_id: {channel_id}, endpoint: {endpoint} }}"
            ),
            Self::Response(r) => fmt::Display::fmt(r, f),
            Self::Data {
                channel_id,
                data: ref buffer,
            } => write!(
                f,
                "Data {{ channel_id: {}, buffer: {} bytes }}",
                channel_id,
                buffer.len()
            ),
            #[cfg(feature = "heartbeat")]
            Self::Ping(id) => write!(f, "ping {id}"),
            #[cfg(feature = "heartbeat")]
            Self::Pong(id) => write!(f, "pong {id}"),
        }
    }
}
