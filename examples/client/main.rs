use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use clap::Parser;

use multiplex::{
    proto::{self, Wire},
    ModeClient, MultiplexerBuilder, MultiplexerClient, Result, Stdio,
};

mod socks;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    bind_addr: std::net::SocketAddr,
}

async fn handle_client(mp: Arc<MultiplexerClient>, mut client: TcpStream) -> Result<()> {
    let mut rx_buffer = [0u8; 256];
    let mut tx_buffer = Vec::with_capacity(32);
    let size = client.read(&mut rx_buffer[..]).await?;

    // Hello
    let (_rest, hello) = socks::Hello::decode(&rx_buffer[..size])?;
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
    let (_rest, request) = socks::Request::decode(&rx_buffer[..size])?;

    let address = match request.addr {
        socks::AddressType::IPv4(ref ip4) => proto::Address::Ipv4(*ip4),
        socks::AddressType::IPv6(ref ip6) => proto::Address::Ipv6(*ip6),
        socks::AddressType::DomainName(ref name) => proto::Address::Name(name.clone()),
    };
    let endpoint = proto::Endpoint::TcpSocket {
        address,
        port: request.port,
    };
    let channel_id = match mp.request_open(endpoint).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                "Cannot open channel {}:{}: {}",
                request.addr,
                request.port,
                e
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
    };

    let response = socks::Response {
        version: socks::Version::Socks5,
        status: socks::Status::Success,
        addr: request.addr,
        port: request.port,
    };
    tx_buffer.clear();
    response.encode_into(&mut tx_buffer);
    client.write_all(&tx_buffer[..]).await?;

    let mut channel = mp.create_channel(client, channel_id)?;
    channel.pipe().await
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

    let (mp, server) = MultiplexerBuilder::<ModeClient, _>::new().build();
    let mp = Arc::new(mp);

    tokio::spawn(async move {
        if let Err(e) = server.serve(stdio).await {
            tracing::error!("Server encountered error: {e}");
        }
    });

    loop {
        let (client, addr) = listener.accept().await?;
        tracing::debug!("New connection from {}", &addr);

        let mp = Arc::clone(&mp);
        tokio::spawn(async move {
            if let Err(e) = handle_client(mp, client).await {
                tracing::error!("Error which client {}: {}", addr, e);
            }
        });
    }
}
