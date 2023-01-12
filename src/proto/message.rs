use std::fmt;

use crate::ChannelId;

use super::request::Request;
use super::response::Response;
use super::Wire;

use nom::branch::alt;
#[cfg(debug_assertions)]
use nom::bytes::streaming::tag;
use nom::bytes::streaming::take;
use nom::combinator::{map, verify};
use nom::error::context;
use nom::number::streaming::{be_u64, be_u8};
use nom::sequence::{preceded, tuple};

/// Messages that can be exchanged
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A request message
    Request(Request),
    /// A response message
    Response(Response),
    /// Data between 2 endpoints
    Data {
        channel_id: ChannelId,
        buffer: Vec<u8>,
    },
}
#[cfg(debug_assertions)]
const MESSAGE_TAG: &[u8; 8] = b"multiplx";
const MESSAGE_TYPE_REQUEST: u8 = 1;
const MESSAGE_TYPE_RESPONSE: u8 = 2;
const MESSAGE_TYPE_DATA: u8 = 3;

fn encode_vec(buffer: &mut Vec<u8>, s: impl AsRef<[u8]>) {
    let data = s.as_ref();
    let size: u64 = data.len().try_into().expect("Buffer too long");
    buffer.extend_from_slice(&size.to_be_bytes()[..]);
    buffer.extend_from_slice(data);
}

fn decode_vec<'i, E>(input: &'i [u8]) -> nom::IResult<&'i [u8], Vec<u8>, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    let (rest, size) = be_u64(input)?;
    let (rest, data) = take(size as usize)(rest)?;
    Ok((rest, data.to_owned()))
}

impl Wire for Message {
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
                buffer: ref data,
            } => {
                buffer.push(MESSAGE_TYPE_DATA);
                buffer.extend_from_slice(&channel_id.to_be_bytes()[..]);
                encode_vec(buffer, data);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let parse_message = alt((
            preceded(
                verify(be_u8, |b| *b == MESSAGE_TYPE_REQUEST),
                map(Request::decode, Self::Request),
            ),
            preceded(
                verify(be_u8, |b| *b == MESSAGE_TYPE_RESPONSE),
                map(Response::decode, Self::Response),
            ),
            preceded(
                verify(be_u8, |b| *b == MESSAGE_TYPE_DATA),
                map(tuple((be_u64, decode_vec)), |(channel_id, data)| {
                    Self::Data {
                        channel_id,
                        buffer: data,
                    }
                }),
            ),
        ));

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

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(ref r) => fmt::Display::fmt(r, f),
            Self::Response(ref r) => fmt::Display::fmt(r, f),
            Self::Data {
                channel_id,
                ref buffer,
            } => write!(
                f,
                "Data {{ channel_id: {}, buffer: {} bytes }}",
                channel_id,
                buffer.len()
            ),
        }
    }
}
