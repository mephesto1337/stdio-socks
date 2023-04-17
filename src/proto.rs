use nom::{
    combinator::{map, map_opt},
    error::context,
    multi::length_data,
    number::streaming::be_u32,
};

use crate::ChannelId;

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
}

impl Wire for String {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let data = self.as_bytes();
        let data_len: u32 = data.len().try_into().expect("Name too long");
        buffer.extend_from_slice(&data_len.to_be_bytes()[..]);
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

impl Wire for Vec<u8> {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let size: u32 = self
            .len()
            .try_into()
            .expect("Vec's length does not fit into u32");
        buffer.extend_from_slice(&size.to_be_bytes()[..]);
        buffer.extend_from_slice(&self[..]);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context("Vec", map(length_data(be_u32), |data: &[u8]| data.to_vec()))(buffer)
    }
}

mod address;
mod endpoint;
mod message;
mod request;
mod response;

pub use address::Address;
pub use endpoint::{EmptyCustom, Endpoint};
pub use message::Message;
pub use request::Request;
pub use response::Response;

impl<C> From<Response<C>> for Message<C> {
    fn from(r: Response<C>) -> Self {
        Self::Response(r)
    }
}

impl<C> From<Request<C>> for Message<C> {
    fn from(r: Request<C>) -> Self {
        Self::Request(r)
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
