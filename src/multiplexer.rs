use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, RwLock},
    {fmt, io},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

use crate::{
    proto::{EmptyCustom, Endpoint, Message, Request, Response, Wire},
    ChannelId, Result,
};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Unpin {}

/// Server part for the multiplexer
pub struct Multiplexer<C = EmptyCustom> {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: RwLock<HashMap<ChannelId, mpsc::Sender<Vec<u8>>>>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<Message<C>>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<std::result::Result<Response<C>, String>>>>,
}

/// A channel to interact with a single client
pub struct Channel<C = EmptyCustom> {
    /// It's identifier
    id: ChannelId,

    /// queue to send data to the endpoint through the multiplexer
    tx: mpsc::Sender<crate::proto::Message<C>>,

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

async fn recv_message<'i, S, C>(stream: &mut S, buffer: &mut Vec<u8>) -> Result<Option<Message<C>>>
where
    C: Wire + fmt::Display + fmt::Debug + Clone,
    S: AsyncRead + Send + Unpin,
{
    let start = buffer.len();
    buffer.reserve(4096);
    let size = stream.read_buf(buffer).await?;
    tracing::trace!("Receive buffer size: {len} (+{size})", len = buffer.len());
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
            tracing::trace!("Resizing buffer, shift of {consumed_bytes}");
            tracing::trace!("{:?} => {msg:?}", &buffer[..consumed_bytes]);
            buffer_memmove(buffer, consumed_bytes);
            assert_eq!(buffer.len(), new_len);
            Ok(Some(msg))
        }
        Err(e) => match e {
            nom::Err::Incomplete(i) => {
                match i {
                    nom::Needed::Unknown => {
                        tracing::debug!("Decode error missing bytes");
                    }
                    nom::Needed::Size(s) => {
                        tracing::debug!("Decode error missing {s} bytes");
                    }
                }
                Ok(None)
            }
            nom::Err::Error(e) => {
                tracing::warn!(
                    "Decode error (recoverable): {err}",
                    err = crate::error::nom_to_owned(nom::Err::Error(e))
                );
                Ok(None)
            }
            nom::Err::Failure(e) => Err(nom::Err::Failure(e).into()),
        },
    }
}

pub type OpenStreamResult<C> = Result<(Box<dyn Stream>, Option<Endpoint<C>>)>;

impl<C> Multiplexer<C>
where
    C: Wire + fmt::Display + fmt::Debug + Clone + Send + 'static,
{
    pub fn create_channel_with_id(
        &self,
        id: ChannelId,
        output: Box<dyn Stream>,
    ) -> Result<Channel<C>> {
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

    pub fn create() -> (Arc<Self>, mpsc::Receiver<crate::proto::Message<C>>) {
        let (tx, rx) = mpsc::channel(64usize);
        (
            Arc::new(Self {
                channels: RwLock::new(HashMap::new()),
                tx,
                queue: Mutex::new(HashMap::new()),
            }),
            rx,
        )
    }

    pub async fn serve<O, S, F>(
        self: Arc<Self>,
        mut stream: S,
        mut rx: mpsc::Receiver<Message<C>>,
        open_stream: &O,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin,
        O: Fn(Endpoint<C>) -> F,
        F: Future<Output = OpenStreamResult<C>> + Send + Unpin,
    {
        let mut rx_buffer = Vec::with_capacity(8192);
        let mut tx_buffer = Vec::with_capacity(8192);

        loop {
            tokio::select! {
                maybe_msg = recv_message(&mut stream, &mut rx_buffer) => {
                    match maybe_msg {
                        Ok(msg) => {
                            if let Some(msg) = msg {
                                tracing::trace!("Received message from stream: {msg}");
                                let me = Arc::clone(&self);
                                me.dispatch_message(msg, open_stream).await?;
                            }
                        },
                        Err(e) => {
                            tracing::error!("Received error from other end: {e}");
                            return Err(e);
                        }
                    }
                },
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            tx_buffer.clear();
                            msg.encode_into(&mut tx_buffer);
                            stream.write_all(&tx_buffer[..]).await?;
                            tracing::trace!("Sending  message to   stream: {msg} ({n} bytes)", n = tx_buffer.len());
                            stream.flush().await?;
                        },
                        None => {
                            tracing::info!("No more message to be received");
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
        }
    }

    pub async fn send(&self, msg: impl Into<Message<C>>) -> Result<()> {
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
                match open_stream(endpoint.clone()).await {
                    Ok((stream, peer_endpoint)) => {
                        let mut channel = self.create_channel_with_id(channel_id, stream)?;
                        self.send(crate::proto::Response::New {
                            channel_id,
                            endpoint: peer_endpoint.unwrap_or(endpoint),
                        })
                        .await?;
                        tokio::spawn(async move {
                            if let Err(e) = channel.pipe().await {
                                tracing::error!("Error wich channel {channel_id}: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        self.send(crate::proto::Response::Error {
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
        let req = crate::proto::Request::Close { channel_id };
        self.request(req).await
    }

    pub async fn request_open(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Endpoint<C>,
    ) -> Result<oneshot::Receiver<std::result::Result<Response<C>, String>>> {
        let req = crate::proto::Request::New {
            channel_id,
            endpoint,
        };
        self.request(req).await
    }

    async fn request(
        self: Arc<Self>,
        request: crate::proto::Request<C>,
    ) -> Result<oneshot::Receiver<std::result::Result<Response<C>, String>>> {
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

impl<C> Drop for Channel<C> {
    fn drop(&mut self) {
        tracing::debug!("Dropping channel {id}", id = self.id);
    }
}
