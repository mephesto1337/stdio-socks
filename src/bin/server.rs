use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};

use multiplex::{Error, Multiplexer, Result, Stdio, Stream};

mod socket {
    include!(concat!(env!("OUT_DIR"), "/socket.rs"));
}

use prost::Message;

async fn open_stream(req: Vec<u8>) -> Result<Box<dyn Stream>> {
    let endpoint = socket::Endpoint::decode(&req[..])?;

    let destination = if let Some(ref d) = endpoint.destination {
        d
    } else {
        return Err(Error::AddressResolution("No endpoint given".to_string()));
    };

    match destination {
        socket::endpoint::Destination::Unix(filename) => {
            let handle = match endpoint.proto() {
                socket::Protocol::Tcp => {
                    let h = tokio::net::UnixStream::connect(filename).await?;
                    Box::new(h) as Box<dyn Stream>
                }
                socket::Protocol::Udp => {
                    return Err(Error::IO(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "UDP is not implemented yet",
                    )));
                }
            };
            Ok(handle)
        }
        socket::endpoint::Destination::Ip(ip) => {
            let port: u16 = ip.port.try_into()?;
            let host = if let Some(ref h) = ip.host {
                h
            } else {
                return Err(Error::AddressResolution("No host given".to_string()));
            };
            let addr: SocketAddr = match host {
                socket::ip_endpoint::Host::Ip4(ip4) => (Ipv4Addr::from(*ip4), port).into(),
                socket::ip_endpoint::Host::Ip6(ip6) => {
                    let mut octects = [0u8; 16];
                    if ip6.len() != octects.len() {
                        return Err(Error::AddressResolution(format!(
                            "IPv6 addresses must be {} bytes long",
                            octects.len()
                        )));
                    }
                    octects.copy_from_slice(&ip6[..]);
                    (Ipv6Addr::from(octects), port).into()
                }
                socket::ip_endpoint::Host::Hostname(ref name) => {
                    if let Some(addr) = tokio::net::lookup_host(format!("{}:{}", name, port))
                        .await?
                        .next()
                    {
                        addr
                    } else {
                        return Err(Error::AddressResolution(format!("Cannot resolv {}", name)));
                    }
                }
            };
            let handle = match endpoint.proto() {
                socket::Protocol::Tcp => {
                    let h = tokio::net::TcpStream::connect(addr).await?;
                    Box::new(h) as Box<dyn Stream>
                }
                socket::Protocol::Udp => {
                    return Err(Error::IO(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "UDP is not implemented yet",
                    )));
                }
            };
            Ok(handle)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let stdio = Stdio::new();
    let (mp, rx) = Multiplexer::create();

    let my_open_stream = move |data| Box::pin(open_stream(data));
    mp.serve(stdio, rx, &my_open_stream).await
}
