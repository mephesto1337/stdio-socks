use std::fmt;
use std::net::SocketAddr;

use super::address::Address;
use super::{decode_string, encode_string, Wire};

use nom::branch::alt;
use nom::combinator::{map, verify};
use nom::error::context;
use nom::multi::length_data;
use nom::number::streaming::{be_u16, be_u32, be_u8};
use nom::sequence::{preceded, tuple};

/// Different endpoints to connect to
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// An UNIX socket (STREAM, not DGRAM)
    UnixSocket { path: String },
    /// A TCP endpoint
    TcpSocket { address: Address, port: u16 },
    /// A Custom endpoint
    Custom { specs: Vec<u8> },
}

impl From<SocketAddr> for Endpoint {
    fn from(s: SocketAddr) -> Self {
        match s {
            SocketAddr::V4(saddr4) => Self::TcpSocket {
                address: (*saddr4.ip()).into(),
                port: saddr4.port(),
            },
            SocketAddr::V6(saddr6) => Self::TcpSocket {
                address: (*saddr6.ip()).into(),
                port: saddr6.port(),
            },
        }
    }
}

const ENDPOINT_TYPE_UNIX: u8 = 1;
const ENDPOINT_TYPE_TCP: u8 = 2;
const ENDPOINT_TYPE_CUSTOM: u8 = 3;

impl Wire for Endpoint {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::UnixSocket { ref path } => {
                buffer.push(ENDPOINT_TYPE_UNIX);
                encode_string(buffer, path);
            }
            Self::TcpSocket { ref address, port } => {
                buffer.push(ENDPOINT_TYPE_TCP);
                address.encode_into(buffer);
                buffer.extend_from_slice(&port.to_be_bytes()[..]);
            }
            Self::Custom { specs } => {
                buffer.push(ENDPOINT_TYPE_CUSTOM);
                let size: u32 = specs.len().try_into().expect("Cannot fit size in u32");
                buffer.extend_from_slice(&size.to_be_bytes()[..]);
                buffer.extend_from_slice(&specs[..]);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "Endpoint",
            alt((
                preceded(
                    verify(be_u8, |b| *b == ENDPOINT_TYPE_UNIX),
                    map(decode_string, |path| Self::UnixSocket { path }),
                ),
                preceded(
                    verify(be_u8, |b| *b == ENDPOINT_TYPE_TCP),
                    map(tuple((Address::decode, be_u16)), |(address, port)| {
                        Self::TcpSocket { address, port }
                    }),
                ),
                preceded(
                    verify(be_u8, |b| *b == ENDPOINT_TYPE_CUSTOM),
                    map(length_data(be_u32), |data: &[u8]| Self::Custom {
                        specs: data.to_vec(),
                    }),
                ),
            )),
        )(buffer)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnixSocket { ref path } => write!(f, "unix://{}", path),
            Self::TcpSocket { ref address, port } => write!(f, "tcp://{}:{}", address, port),
            Self::Custom { ref specs } => {
                f.write_str("custom://0x")?;
                for b in &specs[..] {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }
    }
}
