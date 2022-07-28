use nom::bytes::streaming::take;
use nom::combinator::map_opt;
use nom::number::streaming::be_u32;

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

fn encode_string(buffer: &mut Vec<u8>, s: impl AsRef<str>) {
    let raw_name = s.as_ref().as_bytes();
    let raw_name_len: u32 = raw_name.len().try_into().expect("Name too long");
    buffer.extend_from_slice(&raw_name_len.to_be_bytes()[..]);
    buffer.extend_from_slice(raw_name);
}

fn decode_string<'i, E>(input: &'i [u8]) -> nom::IResult<&'i [u8], String, E>
where
    E: nom::error::ParseError<&'i [u8]>,
{
    let (rest, name_len) = be_u32(input)?;
    let (rest, name) = map_opt(take(name_len as usize), |b| {
        std::str::from_utf8(b).map(|s| s.to_owned()).ok()
    })(rest)?;
    Ok((rest, name))
}

mod address;
mod endpoint;
mod message;
mod request;
mod response;

pub use address::Address;
pub use endpoint::Endpoint;
pub use message::Message;
pub use request::Request;
pub use response::Response;

impl From<Response> for Message {
    fn from(r: Response) -> Self {
        Self::Response(r)
    }
}

impl From<Request> for Message {
    fn from(r: Request) -> Self {
        Self::Request(r)
    }
}

impl From<(ChannelId, Vec<u8>)> for Message {
    fn from(r: (ChannelId, Vec<u8>)) -> Self {
        let (channel_id, buffer) = r;
        Self::Data { channel_id, buffer }
    }
}

impl From<(ChannelId, String)> for Message {
    fn from(r: (ChannelId, String)) -> Self {
        let (channel_id, message) = r;
        Self::Response(Response::Error {
            channel_id,
            message,
        })
    }
}
