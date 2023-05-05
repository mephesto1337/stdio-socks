//! All the structures used to multiplex streams into a single one.
//!
//!
use nom::{
    combinator::{map, map_opt},
    error::context,
    multi::{length_count, length_data},
    number::complete::{be_u32, be_u8},
};

use crate::ChannelId;

/// Trait used for serialization/deserialization
pub trait Wire: Sized {
    /// Encodes it-self into a buffer
    fn encode(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        self.encode_into(&mut buffer);
        buffer
    }

    /// Encodes is-self into specified buffer
    fn encode_into(&self, buffer: &mut Vec<u8>);

    /// Decodes it-self from wire
    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>;

    /// Variant of [`Wire::decode`] with [`nom::error::VerboseError`] as error type
    fn decode_verbose(buffer: &[u8]) -> nom::IResult<&[u8], Self, nom::error::VerboseError<&[u8]>> {
        Self::decode(buffer)
    }
}

impl Wire for String {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let data = self.as_bytes();
        let size: u32 = data.len().try_into().unwrap();
        buffer.extend_from_slice(&size.to_be_bytes()[..]);
        buffer.extend_from_slice(data);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "String",
            map_opt(length_data(be_u32), |bytes: &[u8]| {
                std::str::from_utf8(bytes).ok().map(|s| s.to_owned())
            }),
        )(buffer)
    }
}

impl<T> Wire for Vec<T>
where
    T: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let size: u32 = self.len().try_into().unwrap();
        buffer.extend_from_slice(&size.to_be_bytes()[..]);
        for elt in self {
            elt.encode_into(buffer);
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context("Array", length_count(be_u32, T::decode))(buffer)
    }
}

impl<T> Wire for Option<T>
where
    T: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        if let Some(value) = self.as_ref() {
            buffer.push(1);
            value.encode_into(buffer);
        } else {
            buffer.push(0);
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, has_value) = context("Optional value", be_u8)(buffer)?;
        match has_value {
            0 => Ok((rest, None)),
            1 => map(T::decode, Some)(rest),
            _ => Err(nom::Err::Failure(E::add_context(
                buffer,
                "Invalid value for optional",
                nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
            ))),
        }
    }
}

mod address;
mod endpoint;
mod message;
mod packet;
mod response;

pub use address::Address;
pub use endpoint::{Endpoint, RawCustom};
pub use message::Message;
pub use packet::PacketCodec;
pub use response::Response;

impl<C> From<Response> for Message<C> {
    fn from(r: Response) -> Self {
        Self::Response(r)
    }
}

impl<C> From<(ChannelId, Vec<u8>)> for Message<C> {
    fn from(r: (ChannelId, Vec<u8>)) -> Self {
        let (channel_id, data) = r;
        Self::Data { channel_id, data }
    }
}

impl<C> From<(ChannelId, String)> for Message<C> {
    fn from(r: (ChannelId, String)) -> Self {
        let (channel_id, message) = r;
        Self::Response(Response::Error {
            channel_id,
            message,
        })
    }
}
