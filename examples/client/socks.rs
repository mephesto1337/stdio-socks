use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use nom::branch::alt;
use nom::bytes::streaming::take;
use nom::combinator::{map, map_opt, verify};
use nom::error::context;
use nom::multi::count;
use nom::number::streaming::{be_u16, be_u8};
use nom::sequence::{preceded, tuple};

use multiplex::proto::Wire;

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Version {
    Socks5 = 5,
}

impl Wire for Version {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.push(*self as u8);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "SocksVersion",
            map(verify(be_u8, |b| *b == Self::Socks5 as u8), |_| {
                Self::Socks5
            }),
        )(buffer)
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AuthenticationMethod {
    None = 0,
    // UsernamePassword = 2,
    NotAcceptable = 0xff,
}

impl Wire for AuthenticationMethod {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.push(*self as u8);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "SocksVersion",
            alt((
                map(verify(be_u8, |b| *b == Self::None as u8), |_| Self::None),
                map(verify(be_u8, |b| *b == Self::NotAcceptable as u8), |_| {
                    Self::NotAcceptable
                }),
            )),
        )(buffer)
    }
}
#[derive(Debug)]
pub struct Hello {
    pub version: Version,
    pub methods: Vec<AuthenticationMethod>,
}

impl Wire for Hello {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        self.version.encode_into(buffer);
        buffer.push(
            self.methods
                .len()
                .try_into()
                .expect("Too many available methods"),
        );
        for m in &self.methods {
            m.encode_into(buffer);
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, (version, nmethods)) = tuple((Version::decode, be_u8))(buffer)?;
        let (rest, methods) = count(AuthenticationMethod::decode, nmethods as usize)(rest)?;
        Ok((rest, Self { methods, version }))
    }
}

#[derive(Debug)]
pub struct HelloResponse {
    pub version: Version,
    pub method: AuthenticationMethod,
}

impl Wire for HelloResponse {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        self.version.encode_into(buffer);
        self.method.encode_into(buffer);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, (version, method)) =
            tuple((Version::decode, AuthenticationMethod::decode))(buffer)?;
        Ok((rest, Self { version, method }))
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Command {
    Connect = 1,
}

impl Wire for Command {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.push(*self as u8);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "Socks command",
            map(verify(be_u8, |b| *b == Self::Connect as u8), |_| {
                Self::Connect
            }),
        )(buffer)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddressType {
    IPv4(Ipv4Addr),
    DomainName(String),
    IPv6(Ipv6Addr),
}

fn encode_hostname(buffer: &mut Vec<u8>, name: &str) {
    let name_sz: u8 = name.len().try_into().expect("Name too long");
    buffer.push(name_sz);
    buffer.extend_from_slice(name.as_bytes());
}

fn decode_hostname<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], String, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    let (rest, name_len) = be_u8(buffer)?;
    let (rest, name) = map_opt(take(name_len as usize), |b| {
        std::str::from_utf8(b).map(|s| s.to_owned()).ok()
    })(rest)?;
    Ok((rest, name))
}

impl Wire for AddressType {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        match self {
            Self::IPv4(ref ip4) => {
                buffer.push(1);
                encode_ipv4(ip4, buffer);
            }
            Self::IPv6(ref ip6) => {
                buffer.push(4);
                encode_ipv6(ip6, buffer);
            }
            Self::DomainName(ref name) => {
                buffer.push(3);
                encode_hostname(buffer, name);
            }
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "Socks address",
            alt((
                preceded(
                    verify(be_u8, |b| *b == 1),
                    map(decode_ipv4, |ip4| Self::IPv4(ip4)),
                ),
                preceded(
                    verify(be_u8, |b| *b == 3),
                    map(decode_hostname, |name| Self::DomainName(name)),
                ),
                preceded(
                    verify(be_u8, |b| *b == 4),
                    map(decode_ipv6, |ip6| Self::IPv6(ip6)),
                ),
            )),
        )(buffer)
    }
}

fn encode_ipv4(ip4: &Ipv4Addr, buffer: &mut Vec<u8>) {
    for b in ip4.octets() {
        buffer.push(b);
    }
}

fn decode_ipv4<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Ipv4Addr, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    let mut octets = [0u8; 4];
    let (rest, bytes) = take(4usize)(buffer)?;
    octets.copy_from_slice(bytes);
    Ok((rest, Ipv4Addr::from(octets)))
}

fn encode_ipv6(ip6: &Ipv6Addr, buffer: &mut Vec<u8>) {
    for b in ip6.octets() {
        buffer.push(b);
    }
}

fn decode_ipv6<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Ipv6Addr, E>
where
    E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
{
    let mut octets = [0u8; 16];
    let (rest, bytes) = take(16usize)(buffer)?;
    octets.copy_from_slice(bytes);
    Ok((rest, Ipv6Addr::from(octets)))
}

impl fmt::Display for AddressType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IPv4(ref ip4) => fmt::Display::fmt(ip4, f),
            Self::IPv6(ref ip6) => write!(f, "[{}]", ip6),
            Self::DomainName(ref name) => f.write_str(name),
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub version: Version,
    pub command: Command,
    pub addr: AddressType,
    pub port: u16,
}

impl Wire for Request {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        self.version.encode_into(buffer);
        self.command.encode_into(buffer);
        buffer.push(0);
        self.addr.encode_into(buffer);
        buffer.extend_from_slice(&self.port.to_be_bytes()[..]);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, (version, command, _zero, addr, port)) = tuple((
            Version::decode,
            Command::decode,
            be_u8,
            AddressType::decode,
            be_u16,
        ))(buffer)?;
        Ok((
            rest,
            Self {
                version,
                command,
                addr,
                port,
            },
        ))
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Status {
    Success = 0,
    GeneralFailure = 1,
}

impl Wire for Status {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.push(*self as u8);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "Socks status",
            alt((
                map(verify(be_u8, |b| *b == Self::Success as u8), |_| {
                    Self::Success
                }),
                map(verify(be_u8, |b| *b == Self::GeneralFailure as u8), |_| {
                    Self::GeneralFailure
                }),
            )),
        )(buffer)
    }
}

#[derive(Debug)]
pub struct Response {
    pub version: Version,
    pub status: Status,
    pub addr: AddressType,
    pub port: u16,
}

impl Wire for Response {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        self.version.encode_into(buffer);
        self.status.encode_into(buffer);
        buffer.push(0);
        self.addr.encode_into(buffer);
        buffer.extend_from_slice(&self.port.to_be_bytes()[..]);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, (version, status, _zero, addr, port)) = context(
            "Socks response",
            tuple((
                Version::decode,
                Status::decode,
                be_u8,
                AddressType::decode,
                be_u16,
            )),
        )(buffer)?;
        Ok((
            rest,
            Self {
                version,
                status,
                addr,
                port,
            },
        ))
    }
}
