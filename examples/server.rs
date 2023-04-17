use std::net::SocketAddr;

use multiplex::{self, proto};
use multiplex::{Error, Multiplexer, OpenStreamResult, Result, Stdio, Stream};

async fn open_stream(endpoint: proto::Endpoint) -> OpenStreamResult {
    match endpoint {
        proto::Endpoint::UnixSocket { path } => {
            let handle = tokio::net::UnixStream::connect(path).await?;
            Ok((Box::new(handle) as Box<dyn Stream>, None))
        }
        proto::Endpoint::TcpSocket { address, port } => {
            let addr: SocketAddr = match address {
                proto::Address::Ipv4(ip4) => (ip4, port).into(),
                proto::Address::Ipv6(ip6) => (ip6, port).into(),
                proto::Address::Name(ref name) => {
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
            let handle = tokio::net::TcpStream::connect(addr).await?;
            let endpoint = handle.peer_addr().ok().map(|addr| addr.into());
            Ok((Box::new(handle) as Box<dyn Stream>, endpoint))
        }
        proto::Endpoint::Custom { .. } => Err(Error::IO(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "No custom endpoint defined",
        ))),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    tracing::info!("Server started");
    let stdio = Stdio::new();
    let (mp, rx) = Multiplexer::create("server");

    let my_open_stream = move |data| Box::pin(open_stream(data));
    if let Err(e) = mp.serve(stdio, rx, &my_open_stream).await {
        tracing::error!("Error with main loop on server: {}", e);
    }

    Ok(())
}
