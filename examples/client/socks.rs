use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use bytes::{Buf, BufMut};

use crate::{Error, Result};

pub trait Wire: Sized {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut;
    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf;
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Version {
    Socks5 = 5,
}

impl Wire for Version {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        buf.put_u8(*self as u8);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        match buf.get_u8() {
            5 => Ok(Self::Socks5),
            v => Err(Error::InvalidEnumValue(v as u64, "Version")),
        }
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AuthenticationMethod {
    None = 0,
    // UsernamePassword = 2,
}

impl Wire for AuthenticationMethod {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        buf.put_u8(*self as u8);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        match buf.get_u8() {
            0 => Ok(Self::None),
            // 2 => Ok(Self::UsernamePassword),
            v => Err(Error::InvalidEnumValue(v as u64, "AuthenticationMethod")),
        }
    }
}
#[derive(Debug)]
pub struct Hello {
    pub version: Version,
    pub methods: Vec<AuthenticationMethod>,
}

impl Wire for Hello {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        self.version.serialize(buf)?;
        buf.put_u8(self.methods.len().try_into()?);
        for m in &self.methods {
            m.serialize(buf)?;
        }
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        let version = Version::deserialize(buf)?;
        let nmethods: usize = buf
            .get_u8()
            .try_into()
            .expect("Cannot fit a u8 into a usize?!");
        let methods = (0..nmethods)
            .filter_map(|_| AuthenticationMethod::deserialize(buf).ok())
            .collect::<Vec<_>>();

        Ok(Self { version, methods })
    }
}

#[derive(Debug)]
pub struct HelloResponse {
    pub version: Version,
    pub method: AuthenticationMethod,
}

impl Wire for HelloResponse {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        self.version.serialize(buf)?;
        self.method.serialize(buf)
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        let version = Version::deserialize(buf)?;
        let method = AuthenticationMethod::deserialize(buf)?;

        Ok(Self { version, method })
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Command {
    Connect = 1,
}

impl Wire for Command {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        buf.put_u8(*self as u8);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        match buf.get_u8() {
            1 => Ok(Self::Connect),
            v => Err(Error::InvalidEnumValue(v as u64, "Command")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddressType {
    IPv4(Ipv4Addr),
    DomainName(String),
    IPv6(Ipv6Addr),
}

impl Wire for AddressType {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        match self {
            Self::IPv4(ref ip4) => {
                buf.put_u8(1);
                ip4.serialize(buf)
            }
            Self::IPv6(ref ip6) => {
                buf.put_u8(4);
                ip6.serialize(buf)
            }
            Self::DomainName(ref name) => {
                buf.put_u8(3);
                name.serialize(buf)
            }
        }
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        match buf.get_u8() {
            1 => {
                let ip4 = Ipv4Addr::deserialize(buf)?;
                Ok(Self::IPv4(ip4))
            }
            3 => {
                let name = String::deserialize(buf)?;
                Ok(Self::DomainName(name))
            }
            4 => {
                let ip6 = Ipv6Addr::deserialize(buf)?;
                Ok(Self::IPv6(ip6))
            }
            v => Err(Error::InvalidEnumValue(v as u64, "AddressType")),
        }
    }
}

impl Wire for Ipv4Addr {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        for b in self.octets() {
            buf.put_u8(b);
        }
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        Ok(Self::from(buf.get_u32()))
    }
}

impl Wire for Ipv6Addr {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        for b in self.octets() {
            buf.put_u8(b);
        }
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        Ok(Self::from(buf.get_u128()))
    }
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

impl Wire for String {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        let size: u8 = self.len().try_into()?;
        buf.put_u8(size);
        buf.put_slice(self.as_bytes());
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        let size: usize = buf
            .get_u8()
            .try_into()
            .expect("Cannot convert a u8 into a usize?!");
        let data = buf.chunk()[..size].to_vec();
        buf.advance(size);
        Ok(String::from_utf8(data)?)
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
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        self.version.serialize(buf)?;
        self.command.serialize(buf)?;
        buf.put_u8(0);
        self.addr.serialize(buf)?;
        buf.put_u16(self.port);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        let version = Version::deserialize(buf)?;
        let command = Command::deserialize(buf)?;
        let _zero = buf.get_u8();
        let addr = AddressType::deserialize(buf)?;
        let port = buf.get_u16();
        Ok(Self {
            version,
            command,
            addr,
            port,
        })
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Status {
    Success = 0,
    GeneralFailure = 1,
}

impl Wire for Status {
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        buf.put_u8(*self as u8);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        match buf.get_u8() {
            0 => Ok(Self::Success),
            1 => Ok(Self::GeneralFailure),
            v => Err(Error::InvalidEnumValue(v as u64, "Command")),
        }
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
    fn serialize<B>(&self, buf: &mut B) -> Result<()>
    where
        B: BufMut,
    {
        self.version.serialize(buf)?;
        self.status.serialize(buf)?;
        buf.put_u8(0);
        self.addr.serialize(buf)?;
        buf.put_u16(self.port);
        Ok(())
    }

    fn deserialize<B>(buf: &mut B) -> Result<Self>
    where
        B: Buf,
    {
        let version = Version::deserialize(buf)?;
        let status = Status::deserialize(buf)?;
        let _zero = buf.get_u8();
        let addr = AddressType::deserialize(buf)?;
        let port = buf.get_u16();
        Ok(Self {
            version,
            status,
            addr,
            port,
        })
    }
}
