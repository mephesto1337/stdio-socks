#![deny(missing_docs)]
//! To multiplex a stream, you will need 2 multiplexer instances. One on each side.
//! The "client" side will be the one asking for opening new channels, and the server will open
//! them.
//!
//! The code client side might look like that:
//! ```rust
//! async fn setup_client(stream: TcpStream) {
//!     // Creates both ends of the multiplexer
//!     let (client, server) = MultiplexerBuilder::<ModeClient, _>::new().build();
//!
//!     // Spawn a task for the server part (own the stream and distribute messages to channels)
//!     tokio::spawn(async move { server.server(stream).await; });
//!
//!     // Makes an Arc to move client on multiple tasks
//!     let client = Arc::new(client);
//!
//!     // Accept new streams
//!     let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
//!     loop {
//!         let (s, _) = listener.accept().await.unwrap();
//!
//!         let client = Arc::clone(&client);
//!         tokio::spawn(async move {
//!             // Setup an arbitrary endpoint
//!             let addr: SocketAddr = ("8.8.8.8", 53).into();
//!             let endpoint: Endpoint = addr.into();
//!
//!             // Request remote to open channel
//!             let channel_id = client.request_open(endpoint).await.unwrap();
//!
//!             // Creates our local channel associate with the TcpStream `s`
//!             let mut channel = client.create_channel(s, channel_id).unwrap();
//!
//!             // equivalent of tokio::io::copy_bidirectionnal
//!             channel.pipe().await.unwrap();
//!         });
//!     }
//!
//! }
//! ```
//!
//! The code server side might look like that:
//! ```rust
//! async fn setup_server(stream: TcpStream) {
//!     // Creates both ends of the multiplexer
//!     let server = MultiplexerBuilder::<ModeServer, _>::new(
//!         Box::new(move |ep: Endpoint| connect_to(ep).await?)
//!     ).build();
//!
//!     // Let the server play its role: listening to request and server them.
//!     server.serve(stream).await.unwrap();
//! }
//! ```
pub type ChannelId = u64;

pub mod proto;

mod error;
pub use error::{Error, Result};

mod stdio;
pub use stdio::Stdio;

mod empty;
pub use empty::DevNull;

mod bytes_io;
pub use bytes_io::BytesIO;

mod multiplexer;
pub use multiplexer::{Channel, Config, MultiplexerClient, MultiplexerServer};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Unpin {}

/// Typedef for open_stream return type
pub type OpenStreamResult = Result<Box<dyn Stream>>;

/// Typedef for open_stream callbacks
pub type OpenStreamFn<C> = dyn Fn(proto::Endpoint<C>) -> Pin<Box<dyn Future<Output = OpenStreamResult> + Send + 'static>>
    + Send
    + Sync;

use std::{
    fmt,
    future::Future,
    io,
    marker::PhantomData,
    pin::Pin,
    sync::{atomic::AtomicU64, Arc},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::Receiver,
};

pub struct MultiplexerBuilder<M, C = proto::RawCustom> {
    _mode: PhantomData<M>,
    mp: Arc<multiplexer::Multiplexer<C>>,
    rx: Receiver<proto::Message<C>>,
    open_stream: Option<Box<OpenStreamFn<C>>>,
}

async fn open_stream<C>(_: proto::Endpoint<C>) -> OpenStreamResult {
    Err(Error::IO(io::Error::new(
        io::ErrorKind::Unsupported,
        "Operation not supported",
    )))
}

pub struct ModeClient;
pub struct ModeServer;

impl<C> MultiplexerBuilder<ModeClient, C>
where
    C: proto::Wire + fmt::Display + fmt::Debug + Send + 'static,
{
    pub fn new() -> Self {
        Self::new_with_config(Config::default())
    }

    pub fn new_with_config(config: Config) -> Self {
        let (mp, rx) = multiplexer::Multiplexer::create_with_config(config);
        Self {
            mp,
            rx,
            open_stream: None,
            _mode: PhantomData,
        }
    }

    pub fn build(self) -> (MultiplexerClient<C>, MultiplexerServer<C>) {
        (
            MultiplexerClient {
                mp: Arc::clone(&self.mp),
                channel_id: AtomicU64::new(0),
            },
            MultiplexerServer {
                open_stream: Box::new(move |ep| Box::pin(open_stream(ep))),
                mp: self.mp,
                rx: self.rx,
            },
        )
    }
}

impl<C> MultiplexerBuilder<ModeServer, C>
where
    C: proto::Wire + fmt::Display + fmt::Debug + Send + 'static,
{
    pub fn new(open_stream: Box<OpenStreamFn<C>>) -> Self {
        Self::new_with_config(open_stream, Config::default())
    }

    pub fn new_with_config(open_stream: Box<OpenStreamFn<C>>, config: Config) -> Self {
        let (mp, rx) = multiplexer::Multiplexer::create_with_config(config);
        Self {
            mp,
            rx,
            open_stream: Some(open_stream),
            _mode: PhantomData,
        }
    }

    pub fn build(mut self) -> MultiplexerServer<C> {
        MultiplexerServer {
            open_stream: self.open_stream.take().unwrap(),
            mp: self.mp,
            rx: self.rx,
        }
    }
}
