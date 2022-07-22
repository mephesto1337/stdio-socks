use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

use tokio::io::{duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};

use prost::Message;

use crate::{ChannelId, Result};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Sync + Unpin {}

impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Sync + Unpin {}

/// Server part for the multiplexer
pub struct Multiplexer {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: HashMap<ChannelId, DuplexStream>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<crate::proto::Message>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<std::result::Result<(), String>>>>,

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
    pipe: DuplexStream,

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

    assert!(position < buffer.len());
    let dst = buffer.as_mut_ptr();
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
        std::intrinsics::copy(src, dst, position);

        // SAFETY: position < buffer.len()
        buffer.set_len(position);
    }
}

async fn recv_message<S>(
    name: &str,
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> Result<Option<crate::proto::message::Msg>>
where
    S: AsyncRead + Send + Sync + Unpin,
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
    let mut cursor = std::io::Cursor::new(&buffer[..]);
    match crate::proto::Message::decode(&mut cursor) {
        Ok(msg) => {
            if let Some(msg) = msg.msg {
                let position: usize = cursor
                    .position()
                    .try_into()
                    .expect("Cannot convert u64 to usize");
                // forget already read elements and shift the rest to the beginnig
                tracing::trace!("Resizing buffer, shift of {}", position);
                buffer_memmove(buffer, position);

                Ok(Some(msg))
            } else {
                Ok(None)
            }
        }
        Err(e) => {
            tracing::debug!(
                "[{}] Decode error (possible unterminated read): {}",
                name,
                e
            );
            Ok(None)
        }
    }
}

impl Multiplexer {
    pub fn create_channel_with_id(
        &mut self,
        id: ChannelId,
        output: Box<dyn Stream>,
    ) -> Result<Channel> {
        let (p1, p2) = duplex(8192usize);
        {
            self.channels.insert(id, p2);
        }

        Ok(Channel {
            id,
            tx: self.tx.clone(),
            pipe: p1,
            stream: output,
        })
    }

    pub fn create(name: impl AsRef<str>) -> (Arc<Self>, mpsc::Receiver<crate::proto::Message>) {
        let (tx, rx) = mpsc::channel(64usize);
        (
            Arc::new(Self {
                channels: HashMap::new(),
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
        S: AsyncRead + AsyncWrite + Send + Sync + Unpin,
        O: Fn(Vec<u8>) -> F,
        F: Future<Output = Result<Box<dyn Stream>>> + Send + Sync + Unpin,
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
                            return Err(e.into());
                        }
                    }
                },
                maybe_msg = rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            tracing::trace!("[{}] Sending  message to   stream: {}", &self.name, &msg);
                            tx_buffer.clear();
                            msg.encode(&mut tx_buffer)?;
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
        &mut self,
        message: crate::proto::message::Msg,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Vec<u8>) -> F,
        F: Future<Output = Result<Box<dyn Stream>>> + Send + Sync + Unpin,
    {
        match message {
            crate::proto::message::Msg::Request(r) => {
                if let Some(r) = r.request {
                    self.dispatch_request(r, open_stream).await
                } else {
                    tracing::warn!("[{}] Empty request received", &self.name);
                    Ok(())
                }
            }
            crate::proto::message::Msg::Response(r) => {
                if let Some(r) = r.response {
                    self.dispatch_response(r).await
                } else {
                    tracing::warn!("[{}] Empty response received", &self.name);
                    Ok(())
                }
            }
            crate::proto::message::Msg::Data(data) => {
                if let Some(pipe) = self.channels.get_mut(&data.channel_id) {
                    if let Err(e) = pipe.write_all(&data.buffer[..]).await {
                        tracing::error!("Cannot write to channel {}: {}", data.channel_id, e);
                    }
                } else {
                    tracing::warn!(
                        "[{}] Tried to write into a closed channel ({})",
                        &self.name,
                        data.channel_id
                    );

                    self.send((
                        data.channel_id,
                        format!("Channel {} is closed", data.channel_id),
                    ))
                    .await?;
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

    async fn dispatch_response(
        self: Arc<Self>,
        response: crate::proto::response::Response,
    ) -> Result<()> {
        match response {
            crate::proto::response::Response::Error(e) => {
                tracing::error!(
                    "[{}] Got error on channel {}: {}",
                    &self.name,
                    e.channel_id,
                    e.error
                );
                self.channels.lock()?.remove(&e.channel_id);
            }
            crate::proto::response::Response::New(n) => {
                tracing::info!("[{}] Channel {} created", &self.name, n.channel_id);
            }
            crate::proto::response::Response::Close(c) => {
                tracing::info!("[{}] Channel {} closed", &self.name, c.channel_id);
            }
        }

        Ok(())
    }

    async fn dispatch_request<F, O>(
        self: Arc<Self>,
        request: crate::proto::request::Request,
        open_stream: &O,
    ) -> Result<()>
    where
        O: Fn(Vec<u8>) -> F,
        F: Future<Output = Result<Box<dyn Stream>>> + Send + Sync + Unpin,
    {
        match request {
            crate::proto::request::Request::New(new) => {
                let stream = open_stream(new.endpoint).await?;
                self.send(crate::proto::ResponseChannelNew {
                    channel_id: new.channel_id,
                })
                .await?;
                let mut channel = self.create_channel_with_id(new.channel_id, stream)?;
                tokio::spawn(async move {
                    if let Err(e) = channel.pipe().await {
                        tracing::error!(
                            "[{}] Error wich channel {}: {}",
                            &self.name,
                            new.channel_id,
                            e
                        );
                    }
                });
            }
            crate::proto::request::Request::Close(close) => {
                let mut channels = self.channels.lock()?;
                if channels.remove(&close.channel_id).map(|_| ()).is_some() {
                    tracing::debug!("[{}] Closing channel {}", &self.name, close.channel_id);
                    self.send(crate::proto::ResponseChannelNew {
                        channel_id: close.channel_id,
                    })
                    .await?;
                } else {
                    tracing::debug!(
                        "[{}] Closing unexisting channel {}",
                        &self.name,
                        close.channel_id
                    );
                    self.send((close.channel_id, format!("Unknown channel ID")))
                        .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn request_close(
        self: Arc<Self>,
        channel_id: ChannelId,
    ) -> Result<oneshot::Receiver<std::result::Result<(), String>>> {
        let req =
            crate::proto::request::Request::Close(crate::proto::RequestChannelClose { channel_id });
        self.request(req).await
    }

    pub async fn request_open(
        self: Arc<Self>,
        channel_id: ChannelId,
        endpoint: Vec<u8>,
    ) -> Result<oneshot::Receiver<std::result::Result<(), String>>> {
        let req = crate::proto::request::Request::New(crate::proto::RequestChannelNew {
            channel_id,
            endpoint,
        });
        self.request(req).await
    }

    async fn request(
        self: Arc<Self>,
        request: crate::proto::request::Request,
    ) -> Result<oneshot::Receiver<std::result::Result<(), String>>> {
        let channel_id = match request {
            crate::proto::request::Request::New(ref n) => n.channel_id,
            crate::proto::request::Request::Close(ref c) => c.channel_id,
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
        let mut stream_buffer = [0u8; 8192];
        let mut pipe_buffer = [0u8; 8192];

        loop {
            tokio::select! {
                res = self.stream.read(&mut stream_buffer[..]) => {
                    match res {
                        Ok(0) => {
                            tracing::debug!("Stream has ended");
                            break;
                        },
                        Ok(n) => {
                            let msg = (self.id, stream_buffer[..n].to_vec());
                            if let Err(e) = self.tx.send(msg.into()).await {
                                tracing::error!("Error while sending into channel: {:?}", &e);
                                return Err(e.into());
                            }
                        },
                        Err(e) => {
                            tracing::warn!("Got error while reading stream: {}", &e);
                            return Err(e.into());
                        }
                    }
                },
                res = self.pipe.read(&mut pipe_buffer[..]) => {
                    match res {
                        Ok(0) => {
                            tracing::debug!("Pipe has ended");
                            break;
                        },
                        Ok(n) => {
                            self.stream.write_all(&pipe_buffer[..n]).await?;
                        },
                        Err(e) => {
                            tracing::warn!("Got error while reading pipe: {}", &e);
                            return Err(e.into());
                        }
                    }
                }
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
