use std::{fmt, net::SocketAddr, ops::Deref};

use super::{address::Address, Wire};

use nom::{
    combinator::map,
    error::context,
    number::streaming::{be_u16, be_u8},
    sequence::tuple,
};

/// Dummy type to avoid unnecessary parsing of Custom endpints
#[derive(Debug, Default)]
pub struct RawCustom(Vec<u8>);

impl Deref for RawCustom {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for RawCustom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// Different endpoints to connect to
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint<C = RawCustom> {
    /// An UNIX socket (STREAM, not DGRAM). Can be built from [`std::string::String`]
    UnixSocket {
        /// Path to the unix socket or windows named pipe
        path: String,
    },
    /// A TCP endpoint. Can be built from [`std::net::SocketAddr`]
    TcpSocket {
        /// Address to connect to
        address: Address,
        /// Port to connect to
        port: u16,
    },
    /// A Custom endpoint
    Custom(C),
}

impl<C> From<SocketAddr> for Endpoint<C> {
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

impl<C> From<String> for Endpoint<C> {
    fn from(path: String) -> Self {
        Self::UnixSocket { path }
    }
}

const ENDPOINT_TYPE_UNIX: u8 = 1;
const ENDPOINT_TYPE_TCP: u8 = 2;
const ENDPOINT_TYPE_CUSTOM: u8 = 3;

impl<C> Wire for Endpoint<C>
where
    C: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::UnixSocket { ref path } => {
                buffer.push(ENDPOINT_TYPE_UNIX);
                path.encode_into(buffer)
            }
            Self::TcpSocket { ref address, port } => {
                buffer.push(ENDPOINT_TYPE_TCP);
                address.encode_into(buffer);
                buffer.extend_from_slice(&port.to_be_bytes()[..]);
            }
            Self::Custom(ref c) => {
                buffer.push(ENDPOINT_TYPE_CUSTOM);
                c.encode_into(buffer);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, endpoint_type) = be_u8(buffer)?;

        match endpoint_type {
            ENDPOINT_TYPE_UNIX => context(
                "Endpoint::UnixSocket",
                map(String::decode, |path| Self::UnixSocket { path }),
            )(rest),

            ENDPOINT_TYPE_TCP => context(
                "Endpoint::TcpSocket",
                map(tuple((Address::decode, be_u16)), |(address, port)| {
                    Self::TcpSocket { address, port }
                }),
            )(rest),

            ENDPOINT_TYPE_CUSTOM => context("Endpoint::Custom", map(C::decode, Self::Custom))(rest),

            _ => Err(nom::Err::Failure(E::add_context(
                buffer,
                "Invalid endpoint type",
                nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
            ))),
        }
    }
}

impl Wire for RawCustom {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.extend_from_slice(&self.0[..]);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        Ok((&b""[..], Self(buffer.to_vec())))
    }
}

impl<C> fmt::Display for Endpoint<C>
where
    C: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnixSocket { ref path } => write!(f, "unix://{}", path),
            Self::TcpSocket { ref address, port } => write!(f, "tcp://{}:{}", address, port),
            Self::Custom(ref c) => write!(f, "custom://{c}"),
        }
    }
}
