use std::{io, net::SocketAddr};

use multiplex::{
    proto, Error, ModeServer, MultiplexerBuilder, OpenStreamResult, Result, Stdio, Stream,
};

async fn open_stream(endpoint: proto::Endpoint) -> OpenStreamResult {
    match endpoint {
        proto::Endpoint::UnixSocket { path } => {
            let handle = tokio::net::UnixStream::connect(path).await?;
            Ok(Box::new(handle) as Box<dyn Stream>)
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
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("Cannot resolv {}", name),
                        )
                        .into());
                    }
                }
            };
            let handle = tokio::net::TcpStream::connect(addr).await?;
            Ok(Box::new(handle) as Box<dyn Stream>)
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

    let server =
        MultiplexerBuilder::<ModeServer, _>::new(Box::new(move |data| Box::pin(open_stream(data))))
            .build();

    if let Err(e) = server.serve(stdio).await {
        tracing::error!("Error with main loop on server: {}", e);
    }

    Ok(())
}
