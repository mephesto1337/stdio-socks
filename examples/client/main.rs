use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use clap::Parser;

use multiplex::proto::{self, Wire};
use multiplex::{ChannelId, Error, Multiplexer, Result, Stdio, Stream};

mod socks;

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
    let (_rest, hello) = socks::Hello::decode(&mut &rx_buffer[..size])?;
    if !hello.methods.contains(&socks::AuthenticationMethod::None) {
        let response = socks::HelloResponse {
            version: socks::Version::Socks5,
            method: socks::AuthenticationMethod::NotAcceptable,
        };
        tx_buffer.clear();
        response.encode_into(&mut tx_buffer);
        client.write_all(&tx_buffer[..]).await?;

        return Ok(());
    }
    let response = socks::HelloResponse {
        version: socks::Version::Socks5,
        method: socks::AuthenticationMethod::None,
    };

    tx_buffer.clear();
    response.encode_into(&mut tx_buffer);
    client.write_all(&tx_buffer[..]).await?;

    // Connect request
    let size = client.read(&mut rx_buffer[..]).await?;
    let (_rest, request) = socks::Request::decode(&mut &rx_buffer[..size])?;

    let address = match request.addr {
        socks::AddressType::IPv4(ref ip4) => proto::Address::Ipv4(ip4.clone()),
        socks::AddressType::IPv6(ref ip6) => proto::Address::Ipv6(ip6.clone()),
        socks::AddressType::DomainName(ref name) => proto::Address::Name(name.clone()),
    };
    let endpoint = proto::Endpoint::TcpSocket {
        address,
        port: request.port,
    };
    let rx = Arc::clone(&mp).request_open(channel_id, endpoint).await?;
    let result = match rx.await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Cannot receive result from queue: {}", e);
            return Ok(());
        }
    };
    if let Err(why) = result {
        tracing::error!(
            "Cannot open channel {}:{}: {}",
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
        response.encode_into(&mut tx_buffer);
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
    response.encode_into(&mut tx_buffer);
    client.write_all(&tx_buffer[..]).await?;

    let mut channel = mp.create_channel_with_id(channel_id, Box::new(client) as Box<dyn Stream>)?;
    channel.pipe().await
}

async fn open_stream(_: proto::Endpoint) -> Result<Box<dyn Stream>> {
    Err(Error::IO(io::Error::new(
        io::ErrorKind::Unsupported,
        "No operation supported in client mode",
    )))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();

    let listener = TcpListener::bind(&args.bind_addr).await?;
    let stdio = Stdio::new();
    let (mp, rx) = Multiplexer::create("client");
    let next_channel_id = AtomicU64::new(0);

    let mp_server = Arc::clone(&mp);
    let my_open_stream = move |data| Box::pin(open_stream(data));
    tokio::spawn(async move {
        let _ = mp_server.serve(stdio, rx, &my_open_stream).await;
    });

    loop {
        let (client, addr) = listener.accept().await?;
        tracing::debug!("New connection from {}", &addr);
        let mp = Arc::clone(&mp);
        let channel_id = next_channel_id.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            tracing::info!("New task for {} with ID {}", addr, channel_id);
            if let Err(e) = handshake(mp, client, channel_id).await {
                tracing::error!("Error which client {}: {}", addr, e);
            }
        });
    }
}
