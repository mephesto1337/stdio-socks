use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex, RwLock};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::proto::{Endpoint, Message, Response, Wire};
use crate::{ChannelId, Result};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Unpin {}

/// Server part for the multiplexer
pub struct Multiplexer {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: RwLock<HashMap<ChannelId, mpsc::Sender<Vec<u8>>>>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<crate::proto::Message>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<std::result::Result<Response, String>>>>,

    /// Name of this multiplexer (mostly used to identify logs lines)
    name: String,
}

/// A channel to interact with a single client
pub struct Channel {
    /// It's identifier
    id: ChannelId,

    /// queue to send data to the endpoint through the multiplexer
    tx: mpsc::Sender<crate::proto::Message>,

    /// queue to receive message
    rx: mpsc::Receiver<Vec<u8>>,

    /// Stream associated
    stream: Box<dyn Stream>,
}

fn buffer_memmove<T>(buffer: &mut Vec<T>, position: usize) {
    if position == 0 {
        return;
    } else if position == buffer.len() {
        buffer.clear();
        return;
    }

    let dst = buffer.as_mut_ptr();
    assert!(position < buffer.len());
    let new_len = buffer.len() - position;
    unsafe {
        // SAFETY: we checked that position is within the slice's bounds
        let src = dst.offset(
            position
                .try_into()
                .expect("Cannot fit a usize into a isize"),
        );
        // SAFETY:
        // * src is valid for reads  of `position * sizeof::<T>()`
        // * dst is valid for writes of `position * sizeof::<T>()`
        // * both src and dst are properly aligned
        std::intrinsics::copy(src, dst, new_len);

        // SAFETY: position < buffer.len()
        buffer.set_len(new_len);
    }
}

async fn recv_message<'i, S>(
    name: &str,
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Option<crate::proto::Message>>
where
    S: AsyncRead + Send + Unpin,
{
    let start = buffer.len();
    buffer.reserve(4096);
    let size = stream.read_buf(buffer).await?;
    tracing::trace!(
        "[{}] Receive buffer size: {} (+{})",
        name,
        buffer.len(),
        size
    );
    if size == 0 {
        return Err(
            io::Error::new(io::ErrorKind::BrokenPipe, "Remote end has closed stream").into(),
        );
    }
    assert_eq!(buffer.len(), start + size);
    match Message::decode::<nom::error::VerboseError<&[u8]>>(&buffer[..]) {
        Ok((rest, msg)) => {
            let new_len = rest.len();
            let consumed_bytes = buffer.len() - rest.len();
            tracing::trace!("Resizing buffer, shift of {}", consumed_bytes);
            tracing::trace!("{:?} => {:?}", &buffer[..consumed_bytes], &msg);
            buffer_memmove(buffer, consumed_bytes);
            assert_eq!(buffer.len(), new_len);
            Ok(Some(msg))
        }
        Err(e) => match e {
            nom::Err::Incomplete(i) => {
                match i {
                    nom::Needed::Unknown => {
                        tracing::debug!("[{}] Decode error missing bytes", name);
                    }
                    nom::Needed::Size(s) => {
                        tracing::debug!("[{}] Decode error missing {} bytes", name, s);
                    }
                }
                Ok(None)
            }
            nom::Err::Error(e) => {
                tracing::warn!(
                    "[{}] Decode error (recoverable): {}",
                    name,
                    crate::error::nom_to_owned(nom::Err::Error(e))
                );
                Ok(None)
            }
            nom::Err::Failure(e) => Err(nom::Err::Failure(e).into()),
        },
    }
}

pub type OpenStreamResult = Result<(Box<dyn Stream>, Option<Endpoint>)>;

impl Multiplexer {
    pub fn create_channel_with_id(
        &self,
        id: ChannelId,
        output: Box<dyn Stream>,
    ) -> Result<Channel> {
        let (tx, rx) = mpsc::channel(64usize);
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

    pub fn create(name: impl AsRef<str>) -> (Arc<Self>, mpsc::Receiver<crate::proto::Message>) {
        let (tx, rx) = mpsc::channel(64usize);
        (
            Arc::new(Self {
                channels: RwLock::new(HashMap::new()),
                tx,
                queue: Mutex::new(HashMap::new()),
                name: name.as_ref().to_owned(),
            }),
            rx,
        )
    }

    pub async fn serve<O, S, F>(
        self: Arc<Self>,
        mut stream: S,
        mut rx: mpsc::Receiver<crate::proto::Message>,
        open_stream: &O,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin,
        O: Fn(Endpoint) -> F,
        F: Future<Output = OpenStreamResult> + Send + Unpin,
    {
        let mut rx_buffer = Vec::with_capacity(8192);
        let mut tx_buffer = Vec::with_capacity(8192);

        loop {
            tokio::select! {
                maybe_msg = recv_message(&self.name, &mut stream, &mut rx_buffer) => {
                    match maybe_msg {
                        Ok(msg) => {
                            if let Some(msg) = msg {
                                tracing::trace!("[{}] Received message from stream: {}", &self.name, &msg);
                                let me = Arc::clone(&self);
                                me.dispatch_message(msg, open_stream).await?;
                            }
                        },
                        Err(e) => {
                            tracing::error!("[{}] Received error from other end: {}", &self.name, &e);
                            return Err(e);
                        }
                    }
                },
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            tracing::trace!("[{}] Sending  message to   stream: {}", &self.name, &msg);
                            tx_buffer.clear();
                            msg.encode_into(&mut tx_buffer);
                            stream.write_all(&tx_buffer[..]).await?;
                            stream.flush().await?;
                        },
                        None => {
                            tracing::info!("[{}] No more message to be received", &self.name);
                            break;
                        }
                    }
                },
            }
        }

        Ok(())
    }

    async fn dispatch_message<O, F>(
        self: Arc<Self>,
        message: Message,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint) -> F,
        F: Future<Output = OpenStreamResult> + Send + Unpin,
    {
        match message {
            Message::Request(r) => self.dispatch_request(r, open_stream).await,
            Message::Response(r) => self.dispatch_response(r).await,
            Message::Data { channel_id, buffer } => {
                let tx = {
                    let channels = self.channels.read()?;
                    if let Some(tx) = channels.get(&channel_id).cloned() {
                        tracing::trace!("[{}] Got TX for channel {}", &self.name, &channel_id);
                        Ok(tx)
                    } else {
                        tracing::warn!(
                            "[{}] Tried to write into a closed channel ({})",
                            &self.name,
                            channel_id
                        );

                        Err((channel_id, format!("Channel {} is closed", channel_id)))
                    }
                };
                match tx {
                    Ok(tx) => tx.send(buffer).await?,
                    Err(e) => self.send(e).await?,
                }
                Ok(())
            }
        }
    }

    pub async fn send(&self, msg: impl Into<crate::proto::Message>) -> Result<()> {
        if let Err(e) = self.tx.send(msg.into()).await {
            tracing::error!("[{}] Could not send {:?} to remote", &self.name, e);
            Err(e.into())
        } else {
            Ok(())
        }
    }

    async fn dispatch_response(self: Arc<Self>, response: crate::proto::Response) -> Result<()> {
        let (channel_id, result) = match response {
            crate::proto::Response::Error {
                channel_id,
                message,
            } => (channel_id, Err(message)),
            crate::proto::Response::New {
                channel_id,
                endpoint,
            } => (
                channel_id,
                Ok(Response::New {
                    channel_id,
                    endpoint,
                }),
            ),
            crate::proto::Response::Close { channel_id } => (channel_id, Ok(response)),
        };
        let maybe_tx = {
            let mut queue = self.queue.lock()?;
            queue.remove(&channel_id)
        };

        if let Some(tx) = maybe_tx {
            if let Err(e) = tx.send(result) {
                tracing::error!(
                    "[{}] Could send back result on channel {}: {:?}",
                    &self.name,
                    channel_id,
                    e
                );
            }
        } else {
            tracing::warn!(
                "[{}] No handler for response on channel {}",
                &self.name,
                channel_id
            );
        }

        Ok(())
    }

    async fn dispatch_request<F, O>(
        self: Arc<Self>,
        request: crate::proto::Request,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Endpoint) -> F,
        F: Future<Output = OpenStreamResult> + Send + Unpin,
    {
        match request {
            crate::proto::Request::New {
                channel_id,
                endpoint,
            } => {
                tracing::info!("Channel {} associated with {}", channel_id, &endpoint);
                let (stream, peer_endpoint) = open_stream(endpoint.clone()).await?;
                let mut channel = self.create_channel_with_id(channel_id, stream)?;
                self.send(crate::proto::Response::New {
                    channel_id,
                    endpoint: peer_endpoint.unwrap_or(endpoint),
                })
                .await?;
                tokio::spawn(async move {
                    if let Err(e) = channel.pipe().await {
                        tracing::error!(
                            "[{}] Error wich channel {}: {}",
                            &self.name,
                            channel_id,
                            e
                        );
                    }
                });
            }
            crate::proto::Request::Close { channel_id } => {
                let maybe_tx = {
                    let mut channels = self.channels.write()?;
                    channels.remove(&channel_id).map(|_| ())
                };
                if maybe_tx.is_some() {
                    tracing::debug!("[{}] Closing channel {}", &self.name, channel_id);
                    self.send(crate::proto::Response::Close { channel_id })
                        .await?;
                } else {
                    tracing::debug!("[{}] Closing unexisting channel {}", &self.name, channel_id);
                    self.send((channel_id, format!("Unknown channel ID 0x{:x}", channel_id)))
                        .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn request_close(
        self: Arc<Self>,
        channel_id: ChannelId,
    ) -> Result<oneshot::Receiver<std::result::Result<Response, String>>> {
        let req = crate::proto::Request::Close { channel_id };
        self.request(req).await
    }

    pub async fn request_open(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Endpoint,
    ) -> Result<oneshot::Receiver<std::result::Result<Response, String>>> {
        let req = crate::proto::Request::New {
            channel_id,
            endpoint,
        };
        self.request(req).await
    }

    async fn request(
        self: Arc<Self>,
        request: crate::proto::Request,
    ) -> Result<oneshot::Receiver<std::result::Result<Response, String>>> {
        let channel_id = match request {
            crate::proto::Request::New { channel_id, .. } => channel_id,
            crate::proto::Request::Close { channel_id } => channel_id,
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

impl Channel {
    pub async fn pipe(&mut self) -> Result<()> {
        let mut buffer = [0u8; 8192];

        loop {
            tokio::select! {
                res = self.stream.read(&mut buffer[..]) => {
                    match res {
                        Ok(0) => {
                            tracing::debug!("Stream has ended");
                            break;
                        },
                        Ok(n) => self.tx.send((self.id, buffer[..n].to_vec()).into()).await?,
                        Err(e) => {
                            tracing::warn!("Got error while reading stream: {}", &e);
                            return Err(e.into());
                        }
                    }
                },
                maybe_data = self.rx.recv() => {
                    match maybe_data {
                        Some(data) => self.stream.write_all(&data[..]).await?,
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

impl Drop for Channel {
    fn drop(&mut self) {
        let prog = std::env::args().next().unwrap();
        tracing::debug!("[{}] Dropping channel {}", prog, self.id);
    }
}
