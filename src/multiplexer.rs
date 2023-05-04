use std::{
    collections::HashMap,
    fmt,
    future::Future,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    time,
};

use crate::{
    proto::{Endpoint, Message, MessageStream, RawCustom, Request, Response, Wire},
    ChannelId, Result,
};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

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

impl Default for Config {
    fn default() -> Self {
        Self {
            #[cfg(feature = "heartbeat")]
            heartbeat: 4,

            #[cfg(feature = "heartbeat")]
            ping_id: std::sync::atomic::AtomicU64::new(0),

            channel_size: 64,
        }
    }
}

impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Unpin {}

/// Server part for the multiplexer
pub struct Multiplexer<C = RawCustom> {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: RwLock<HashMap<ChannelId, mpsc::Sender<Vec<u8>>>>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<Message<C>>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<std::result::Result<Response<C>, String>>>>,

    /// Configuration
    config: Config,
}

/// A channel to interact with a single client
pub struct Channel<C = RawCustom> {
    /// It's identifier
    id: ChannelId,

    /// queue to send data to the endpoint through the multiplexer
    tx: mpsc::Sender<Message<C>>,

    /// queue to receive message
    rx: mpsc::Receiver<Vec<u8>>,

    /// Stream associated
    stream: Box<dyn Stream>,
}

pub type OpenStreamResult<C> = Result<(Box<dyn Stream>, Option<Endpoint<C>>)>;

impl<C> Multiplexer<C>
where
    C: Wire + fmt::Display + fmt::Debug + Send + 'static,
{
    pub fn create_channel_with_id(
        &self,
        id: ChannelId,
        output: Box<dyn Stream>,
    ) -> Result<Channel<C>> {
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

    pub fn create_with_config(config: Config) -> (Arc<Self>, mpsc::Receiver<Message<C>>) {
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

    pub fn create() -> (Arc<Self>, mpsc::Receiver<Message<C>>) {
        let config = Config::default();
        Self::create_with_config(config)
    }

    pub async fn serve<O, S, F>(
        self: Arc<Self>,
        stream: S,
        mut rx: mpsc::Receiver<Message<C>>,
        open_stream: &O,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin,
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult<C>> + Send + Unpin,
    {
        let mut message_stream = MessageStream::<C, _>::new(stream);

        let sleep_duration;
        #[cfg(feature = "heartbeat")]
        {
            sleep_duration = self.config.heartbeat;
        }
        #[cfg(not(feature = "heartbeat"))]
        {
            sleep_duration = 60;
        }

        loop {
            let sleep = time::sleep(Duration::from_secs(sleep_duration));
            tokio::pin!(sleep);

            tokio::select! {
                maybe_msg = message_stream.next() => {
                    match maybe_msg {
                        Some(msg) => {
                                tracing::trace!("Received message from stream: {msg}");
                                let me = Arc::clone(&self);
                                me.dispatch_message(msg, open_stream).await?;
                        },
                        None => {
                            tracing::info!("Stream has ended");
                            break;
                        }
                    }
                },
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            tracing::trace!("Sending  message to   stream: {msg}");
                            message_stream.send(msg).await?;
                        },
                        None => {
                            tracing::info!("No more message to be received");
                            break;
                        }
                    }
                },
                _ = &mut sleep => {
                    #[cfg(feature = "heartbeat")]
                    {
                        tracing::trace!("Sending  ping    to   stream");
                        let msg = crate::proto::Message::<C>::Ping(self.config.ping_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                        message_stream.send(msg).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn dispatch_message<O, F>(
        self: Arc<Self>,
        message: Message<C>,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult<C>> + Send + Unpin,
    {
        match message {
            Message::Request(r) => self.dispatch_request(r, open_stream).await,
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

    async fn send(&self, msg: impl Into<Message<C>>) -> Result<()> {
        if let Err(e) = self.tx.send(msg.into()).await {
            tracing::error!("Could not send {e:?} to remote");
            Err(e.into())
        } else {
            Ok(())
        }
    }

    async fn dispatch_response(self: Arc<Self>, response: Response<C>) -> Result<()> {
        let (channel_id, result) = match response {
            Response::Error {
                channel_id,
                message,
            } => (channel_id, Err(message)),
            Response::New {
                channel_id,
                endpoint,
            } => (
                channel_id,
                Ok(Response::New {
                    channel_id,
                    endpoint,
                }),
            ),
            Response::Close { channel_id } => (channel_id, Ok(response)),
        };
        let maybe_tx = {
            let mut queue = self.queue.lock()?;
            queue.remove(&channel_id)
        };

        // Gor error on channel, if already present sends EOF into it
        if result.is_err() {
            let maybe_tx = {
                let channels = self.channels.read()?;
                channels.get(&channel_id).cloned()
            };
            if let Some(tx) = maybe_tx {
                tracing::debug!("Sending EOF to channel {channel_id}");
                tx.send(Vec::new()).await?;
            }
        }

        if let Some(tx) = maybe_tx {
            if let Err(e) = tx.send(result) {
                tracing::error!("Could send back result on channel {channel_id}: {e:?}");
            }
        } else {
            tracing::warn!("No handler for response on channel {channel_id}");
        }

        Ok(())
    }

    async fn dispatch_request<F, O>(
        self: Arc<Self>,
        request: Request<C>,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult<C>> + Send + Unpin,
    {
        match request {
            Request::New {
                channel_id,
                endpoint,
            } => {
                tracing::info!("Channel {channel_id} associated with {endpoint}");
                match open_stream(endpoint).await {
                    Ok((stream, peer_endpoint)) => {
                        let mut channel = self.create_channel_with_id(channel_id, stream)?;
                        self.send(Response::New {
                            channel_id,
                            endpoint: peer_endpoint,
                        })
                        .await?;
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
            }
            Request::Close { channel_id } => {
                let maybe_tx = {
                    let mut channels = self.channels.write()?;
                    channels.remove(&channel_id).map(|_| ())
                };
                if maybe_tx.is_some() {
                    tracing::debug!("Closing channel {channel_id}");
                    self.send(Response::Close { channel_id }).await?;
                } else {
                    tracing::debug!("Closing unexisting channel {channel_id}");
                    self.send((channel_id, format!("Unknown channel ID {channel_id}")))
                        .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn request_close(
        self: Arc<Self>,
        channel_id: ChannelId,
    ) -> Result<oneshot::Receiver<std::result::Result<Response<C>, String>>> {
        let req = Request::Close { channel_id };
        self.request(req).await
    }

    pub async fn request_open(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
    ) -> Result<oneshot::Receiver<std::result::Result<Response<C>, String>>> {
        let req = Request::New {
            channel_id,
            endpoint,
        };
        self.request(req).await
    }

    async fn request(
        self: Arc<Self>,
        request: Request<C>,
    ) -> Result<oneshot::Receiver<std::result::Result<Response<C>, String>>> {
        let channel_id = match request {
            Request::New { channel_id, .. } => channel_id,
            Request::Close { channel_id } => channel_id,
        };
        let (tx, rx) = oneshot::channel();
        self.send(request).await?;
        {
            let mut queue = self.queue.lock()?;
            queue.insert(channel_id, tx);
        }
        Ok(rx)
    }
}

impl<C> Channel<C>
where
    C: Wire + std::fmt::Display,
{
    pub async fn pipe(&mut self) -> Result<()> {
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
                        Ok(n) => self.tx.send((self.id, buffer[..n].to_vec()).into()).await?,
                        Err(e) => {
                            tracing::warn!("Got error while reading stream: {e}");
                            return Err(e.into());
                        }
                    }
                },
                maybe_data = self.rx.recv() => {
                    match maybe_data {
                        Some(data) => {
                            tracing::trace!("Writing {n} bytes to channel {id} stream", n = data.len(), id = self.id);
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

impl<C> Drop for Channel<C> {
    fn drop(&mut self) {
        tracing::debug!("Dropping channel {id}", id = self.id);
    }
}
