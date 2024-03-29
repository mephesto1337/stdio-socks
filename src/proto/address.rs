use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use crate::proto::Wire;
use nom::{bytes::streaming::take, combinator::map, error::context, number::streaming::be_u8};

const ADDRESS_TYPE_IPV4: u8 = 1;
const ADDRESS_TYPE_IPV6: u8 = 2;
const ADDRESS_TYPE_NAME: u8 = 3;

/// Different address types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// IPv4 address
    Ipv4(Ipv4Addr),
    /// IPv6 address
    Ipv6(Ipv6Addr),
    /// Hostname
    Name(String),
}

impl From<Ipv4Addr> for Address {
    fn from(ip4: Ipv4Addr) -> Self {
        Self::Ipv4(ip4)
    }
}

impl From<Ipv6Addr> for Address {
    fn from(ip6: Ipv6Addr) -> Self {
        Self::Ipv6(ip6)
    }
}

impl From<IpAddr> for Address {
    fn from(e: IpAddr) -> Self {
        match e {
            IpAddr::V4(ip4) => Self::Ipv4(ip4),
            IpAddr::V6(ip6) => Self::Ipv6(ip6),
        }
    }
}

fn decode_ipv4<'i, E>(input: &'i [u8]) -> nom::IResult<&[u8], Ipv4Addr, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    map(take(4usize), |bytes| {
        let mut octets = [0u8; 4];
        octets.copy_from_slice(bytes);
        Ipv4Addr::from(octets)
    })(input)
}

fn decode_ipv6<'i, E>(input: &'i [u8]) -> nom::IResult<&'i [u8], Ipv6Addr, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    map(take(16usize), |bytes| {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(bytes);
        Ipv6Addr::from(octets)
    })(input)
}

impl Wire for Address {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::Ipv4(ref ip4) => {
                buffer.push(ADDRESS_TYPE_IPV4);
                buffer.extend_from_slice(&ip4.octets()[..]);
            }
            Self::Ipv6(ref ip6) => {
                buffer.push(ADDRESS_TYPE_IPV6);
                buffer.extend_from_slice(&ip6.octets()[..]);
            }
            Self::Name(ref name) => {
                buffer.push(ADDRESS_TYPE_NAME);
                name.encode_into(buffer)
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, address_type) = be_u8(buffer)?;

        match address_type {
            ADDRESS_TYPE_IPV4 => context("Address::Ipv4", map(decode_ipv4, Self::Ipv4))(rest),
            ADDRESS_TYPE_IPV6 => context("Address::Ipv6", map(decode_ipv6, Self::Ipv6))(rest),
            ADDRESS_TYPE_NAME => context("Address::Name", map(String::decode, Self::Name))(rest),

            _ => Err(nom::Err::Failure(E::add_context(
                buffer,
                "Invalid address type",
                nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
            ))),
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ipv4(ref ip4) => fmt::Display::fmt(ip4, f),
            Self::Ipv6(ref ip6) => write!(f, "[{}]", ip6),
            Self::Name(ref name) => fmt::Display::fmt(name, f),
        }
    }
}
