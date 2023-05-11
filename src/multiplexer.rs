use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex, RwLock},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

use crate::{
    proto::{Endpoint, Message, RawCustom, Response, Wire},
    ChannelId, OpenStreamResult, Result, Stream,
};

mod client;
pub use client::MultiplexerClient;
mod server;
pub use server::MultiplexerServer;
mod memory_stream;
pub use memory_stream::MemoryStream;

/// Configuration to use in a multiplex session
#[derive(Debug)]
pub struct Config {
    #[cfg(feature = "heartbeat")]
    /// Heartbeat to send if no data during `heartbeat` previous seconds
    heartbeat: u64,

    #[cfg(feature = "heartbeat")]
    /// Current ping id being sent
    ping_id: std::sync::atomic::AtomicU64,

    /// Channel size to send data. The bigger means that if a channel is sending a lot of data,
    /// then other channel may have to way `channel_size` message to send their own
    channel_size: usize,
}

impl Config {
    #[cfg(feature = "heartbeat")]
    fn get_next_id(&self) -> u64 {
        self.ping_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // By default, nginx closes a websocket after 5 seconds of inactivity.
            #[cfg(feature = "heartbeat")]
            heartbeat: 4,

            #[cfg(feature = "heartbeat")]
            ping_id: std::sync::atomic::AtomicU64::new(0),

            channel_size: 64,
        }
    }
}

/// Server part for the multiplexer
pub(super) struct Multiplexer<C = RawCustom> {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: RwLock<HashMap<ChannelId, mpsc::Sender<Vec<u8>>>>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<Message<C>>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<Response>>>,

    /// Configuration
    config: Config,
}

/// A channel to interact with a single client
pub struct Channel<C, S> {
    /// It's identifier
    id: ChannelId,

    /// queue to send data to the endpoint through the multiplexer
    tx: mpsc::Sender<Message<C>>,

    /// queue to receive message
    rx: mpsc::Receiver<Vec<u8>>,

    /// Stream associated
    stream: S,
}

impl<C> Multiplexer<C>
where
    C: Wire + fmt::Display + Send + 'static,
{
    pub(super) fn create_channel_with_id<S>(
        &self,
        id: ChannelId,
        output: S,
    ) -> Result<Channel<C, S>> {
        let (tx, rx) = mpsc::channel(self.config.channel_size);
        {
            let mut channels = self.channels.write()?;
            channels.insert(id, tx);
        }

        Ok(Channel {
            id,
            tx: self.tx.clone(),
            rx,
            stream: output,
        })
    }

    pub(super) fn create_with_config(config: Config) -> (Arc<Self>, mpsc::Receiver<Message<C>>) {
        let (tx, rx) = mpsc::channel(config.channel_size);
        (
            Arc::new(Self {
                channels: RwLock::new(HashMap::new()),
                tx,
                queue: Mutex::new(HashMap::new()),
                config,
            }),
            rx,
        )
    }

    pub(super) async fn dispatch_message<O, F>(
        self: Arc<Self>,
        message: Message<C>,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult> + Send + Unpin,
    {
        match message {
            Message::RequestOpen {
                channel_id,
                endpoint,
            } => self.dispatch_open(channel_id, endpoint, open_stream).await,
            Message::Response(r) => self.dispatch_response(r).await,
            Message::Data { channel_id, data } => {
                let tx = {
                    let channels = self.channels.read()?;
                    if let Some(tx) = channels.get(&channel_id).cloned() {
                        tracing::trace!("Got TX for channel {channel_id}");
                        Ok(tx)
                    } else {
                        tracing::warn!("Tried to write into a closed channel ({channel_id})");

                        Err((channel_id, format!("Channel {channel_id} is closed")))
                    }
                };
                match tx {
                    Ok(tx) => {
                        if let Err(e) = tx.send(data).await {
                            // Channel is closed, close it
                            tracing::warn!("Tried to write into closed channel {channel_id}: {e}");
                            let mut channels = self.channels.write()?;
                            channels.remove(&channel_id);
                        }
                    }
                    Err(e) => self.send(e).await?,
                }
                Ok(())
            }
            #[cfg(feature = "heartbeat")]
            Message::Ping(id) => self.send(Message::Pong(id)).await,
            #[cfg(feature = "heartbeat")]
            Message::Pong(id) => {
                let current = self
                    .config
                    .ping_id
                    .load(std::sync::atomic::Ordering::Relaxed);
                if id >= current {
                    tracing::warn!("Received a ping response from the future ?! (current={current}, received={id})");
                }
                Ok(())
            }
        }
    }

    pub(super) async fn send(&self, msg: impl Into<Message<C>>) -> Result<()> {
        if let Err(e) = self.tx.send(msg.into()).await {
            tracing::error!("Could not send {e} to remote");
            Err(e.into())
        } else {
            Ok(())
        }
    }

    async fn dispatch_response(self: Arc<Self>, response: Response) -> Result<()> {
        let channel_id = response.get_channel_id();
        let maybe_tx = {
            let mut queue = self.queue.lock()?;
            queue.remove(&channel_id)
        };

        // Gor error on channel, close it
        if let Response::Error { .. } = response {
            let maybe_tx = {
                let mut channels = self.channels.write()?;
                channels.remove(&channel_id)
            };
            if let Some(tx) = maybe_tx {
                tracing::debug!("Sending EOF to channel {channel_id}");
                tx.send(Vec::new()).await?;
            }
        }

        if let Some(tx) = maybe_tx {
            if let Err(e) = tx.send(response) {
                tracing::error!("Could send back result on channel {channel_id}: {e:?}");
            }
        } else {
            tracing::warn!("No handler for response on channel {channel_id}");
        }

        Ok(())
    }

    async fn dispatch_open<F, O>(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult> + Send + Unpin,
    {
        tracing::debug!("Request open channel#{channel_id} on {endpoint}");
        match open_stream(endpoint).await {
            Ok(stream) => {
                let channel = self.create_channel_with_id(channel_id, stream)?;
                self.send(Response::Open { channel_id }).await?;
                tokio::spawn(async move {
                    if let Err(e) = channel.pipe().await {
                        tracing::error!("Error wich channel {channel_id}: {e}");
                    }
                });
            }
            Err(e) => {
                self.send(Response::Error {
                    channel_id,
                    message: format!("{e}"),
                })
                .await?;
            }
        }

        Ok(())
    }

    pub async fn request_open(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
    ) -> Result<oneshot::Receiver<Response>> {
        tracing::debug!("Requesting opening of new channel {channel_id} remotely to {endpoint}");
        let (tx, rx) = oneshot::channel();
        self.send(Message::RequestOpen {
            channel_id,
            endpoint,
        })
        .await?;
        {
            let mut queue = self.queue.lock()?;
            queue.insert(channel_id, tx);
        }
        Ok(rx)
    }
}

impl<C, S> Channel<C, S> {
    /// Get channel's id
    pub fn id(&self) -> ChannelId {
        self.id
    }
}
impl<C> Channel<C, Box<dyn Stream>> {
    /// Equivalent of [`tokio::io::copy_bidirectional`] for channels
    pub async fn pipe(mut self) -> Result<()> {
        let mut buffer = [0u8; 8192];

        loop {
            tokio::select! {
                res = self.stream.read(&mut buffer[..]) => {
                    match res {
                        Ok(0) => {
                            tracing::debug!("Stream has ended, sending empty buffer to channel");
                            // Sending EOF to remote side
                            self.tx.send((self.id, Vec::new()).into()).await?;
                            break;
                        },
                        Ok(n) => {
                            tracing::trace!("Writing {n} bytes from stream to channel {id}", id = self.id);
                            self.tx.send((self.id, buffer[..n].to_vec()).into()).await?
                        }
                        Err(e) => {
                            tracing::warn!("Got error while reading stream: {e}");
                            return Err(e.into());
                        }
                    }
                },
                maybe_data = self.rx.recv() => {
                    match maybe_data {
                        Some(data) => {
                            tracing::trace!("Writing {n} bytes from channel#{id} to stream", n = data.len(), id = self.id);
                            if data.is_empty() {
                                break;
                            }
                            self.stream.write_all(&data[..]).await?;
                        }
                        None => {
                            tracing::warn!("All tx have been dropped");
                            break;
                        }
                    }
                },
            }
        }

        Ok(())
    }
}

impl<C> Channel<C, MemoryStream> {
    /// Replace memory stream of channel with actual stream (NOT already boxed)
    pub async fn replace_stream<S>(self, stream: S) -> std::io::Result<Channel<C, Box<dyn Stream>>>
    where
        S: AsyncWrite + AsyncRead + Send + Unpin + 'static,
    {
        self.replace_stream_boxed(Box::new(stream) as Box<dyn Stream>)
            .await
    }

    /// Replace memory stream of channel with actual stream
    pub async fn replace_stream_boxed(
        self,
        mut stream: Box<dyn Stream>,
    ) -> std::io::Result<Channel<C, Box<dyn Stream>>> {
        stream.write_all(self.stream.get_data()).await?;
        Ok(Channel {
            id: self.id,
            tx: self.tx,
            rx: self.rx,
            stream,
        })
    }
}
