use std::fmt;

use crate::ChannelId;

use super::{endpoint::RawCustom, request::Request, response::Response, Wire};

#[cfg(debug_assertions)]
use nom::bytes::streaming::tag;

use nom::{
    combinator::map,
    error::context,
    number::streaming::{be_u64, be_u8},
    sequence::{preceded, tuple},
};

/// Messages that can be exchanged
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message<C = RawCustom> {
    /// A request message
    Request(Request<C>),
    /// A response message
    Response(Response<C>),
    /// Data between 2 endpoints
    Data {
        channel_id: ChannelId,
        data: Vec<u8>,
    },
    #[cfg(feature = "heartbeat")]
    /// Ping to keep stream open, ping request
    Ping(u64),
    #[cfg(feature = "heartbeat")]
    /// Ping to keep stream open, ping reply
    Pong(u64),
}
#[cfg(debug_assertions)]
const MESSAGE_TAG: &[u8; 8] = b"multiplx";
const MESSAGE_TYPE_REQUEST: u8 = 1;
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
    let (rest, message_type) = be_u8(buffer)?;

    match message_type {
        MESSAGE_TYPE_REQUEST => map(Request::<C>::decode, Message::Request)(rest),
        MESSAGE_TYPE_RESPONSE => map(Response::<C>::decode, Message::Response)(rest),
        MESSAGE_TYPE_DATA => map(tuple((be_u64, Vec::decode)), |(channel_id, data)| {
            Message::Data { channel_id, data }
        })(rest),
        #[cfg(feature = "heartbeat")]
        MESSAGE_TYPE_PING => map(be_u64, Message::<C>::Ping)(rest),
        #[cfg(feature = "heartbeat")]
        MESSAGE_TYPE_PONG => map(be_u64, Message::<C>::Pong)(rest),
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
        #[cfg(debug_assertions)]
        {
            buffer.extend_from_slice(MESSAGE_TAG);
        }

        match self {
            Self::Request(ref request) => {
                buffer.push(MESSAGE_TYPE_REQUEST);
                request.encode_into(buffer);
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
                data.encode_into(buffer);
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
        #[cfg(debug_assertions)]
        {
            context("Message", preceded(tag(&MESSAGE_TAG[..]), parse_message))(buffer)
        }

        #[cfg(not(debug_assertions))]
        {
            context("Message", parse_message)(buffer)
        }
    }
}

impl<C> fmt::Display for Message<C>
where
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(ref r) => fmt::Display::fmt(r, f),
            Self::Response(ref r) => fmt::Display::fmt(r, f),
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
