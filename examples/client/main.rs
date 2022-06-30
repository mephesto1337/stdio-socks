use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use clap::Parser;

use prost::Message;

use multiplex::{ChannelId, Error, Multiplexer, Result, Stdio, Stream};

mod socks;
use socks::Wire;

mod socket {
    include!(concat!(env!("OUT_DIR"), "/socket.rs"));
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, parse(try_from_str))]
    bind_addr: std::net::SocketAddr,
}

async fn handshake(
    mp: Arc<Multiplexer>,
    mut client: TcpStream,
    channel_id: ChannelId,
) -> Result<()> {
    let mut rx_buffer = [0u8; 256];
    let mut tx_buffer = Vec::with_capacity(32);
    let size = client.read(&mut rx_buffer[..]).await?;

    // Hello
    let _hello = socks::Hello::deserialize(&mut &rx_buffer[..size])?;
    let response = socks::HelloResponse {
        version: socks::Version::Socks5,
        method: socks::AuthenticationMethod::None,
    };

    tx_buffer.clear();
    response.serialize(&mut tx_buffer)?;
    client.write_all(&tx_buffer[..]).await?;

    // Connect request
    let size = client.read(&mut rx_buffer[..]).await?;
    let request = socks::Request::deserialize(&mut &rx_buffer[..size])?;

    let host = match request.addr {
        socks::AddressType::IPv4(ref ip4) => {
            socket::ip_endpoint::Host::Ip4(u32::from_be_bytes(ip4.octets()))
        }
        socks::AddressType::IPv6(ref ip6) => socket::ip_endpoint::Host::Ip6(ip6.octets().to_vec()),
        socks::AddressType::DomainName(ref name) => {
            socket::ip_endpoint::Host::Hostname(name.clone())
        }
    };
    let req = socket::Endpoint {
        proto: socket::Protocol::Tcp as i32,
        destination: Some(socket::endpoint::Destination::Ip(socket::IpEndpoint {
            port: request
                .port
                .try_into()
                .expect("Cannot fit a u16 into a u32"),
            host: Some(host),
        })),
    };
    let raw_req = req.encode_to_vec();
    let rx = Arc::clone(&mp).request_open(channel_id, raw_req).await?;
    let result = match rx.await {
        Ok(v) => v,
        Err(e) => {
            log::error!("Cannot received result from queue: {}", e);
            return Ok(());
        }
    };
    if let Err(why) = result {
        log::error!(
            "Cannot open channel to {}:{}: {}",
            request.addr,
            request.port,
            why
        );
        let response = socks::Response {
            version: socks::Version::Socks5,
            status: socks::Status::GeneralFailure,
            addr: request.addr,
            port: request.port,
        };
        tx_buffer.clear();
        response.serialize(&mut tx_buffer)?;
        client.write_all(&tx_buffer[..]).await?;
        return Ok(());
    }

    let response = socks::Response {
        version: socks::Version::Socks5,
        status: socks::Status::Success,
        addr: request.addr,
        port: request.port,
    };
    tx_buffer.clear();
    response.serialize(&mut tx_buffer)?;
    client.write_all(&tx_buffer[..]).await?;

    let mut channel = mp.create_channel_with_id(channel_id, Box::new(client) as Box<dyn Stream>)?;
    channel.pipe().await
}

async fn open_stream(_: Vec<u8>) -> Result<Box<dyn Stream>> {
    Err(Error::IO(io::Error::new(
        io::ErrorKind::Unsupported,
        "No operation supported in client mode",
    )))
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let listener = TcpListener::bind(&args.bind_addr).await?;
    let stdio = Stdio::new();
    let (mp, rx) = Multiplexer::create();
    let next_channel_id = AtomicU64::new(0);

    let mp_server = Arc::clone(&mp);
    let my_open_stream = move |data| Box::pin(open_stream(data));
    tokio::spawn(async move {
        let _ = mp_server.serve(stdio, rx, &my_open_stream).await;
    });

    loop {
        let (client, addr) = listener.accept().await?;
        log::debug!("New connection from {}", addr);
        let mp = Arc::clone(&mp);
        let channel_id = next_channel_id.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            if let Err(e) = handshake(mp, client, channel_id).await {
                log::error!("Error with client: {}", e);
            }
        });
    }
}
