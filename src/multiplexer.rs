use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex, RwLock};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use prost::Message;

use crate::{ChannelId, Result};

/// Trait for AsyncRead + AsyncWrite objects
pub trait Stream: AsyncRead + AsyncWrite + Send + Sync + Unpin {}

impl<T> Stream for T where T: AsyncWrite + AsyncRead + Send + Sync + Unpin {}

/// Server part for the multiplexer
pub struct Multiplexer {
    /// A mapping between a ChannelId (identifying a destination) and its Sender
    channels: RwLock<HashMap<ChannelId, mpsc::Sender<crate::proto::Flow>>>,

    /// A Sender to give to each worker
    tx: mpsc::Sender<crate::proto::Message>,

    /// Waiting responses
    queue: Mutex<HashMap<ChannelId, oneshot::Sender<std::result::Result<(), String>>>>,

    /// Name of this multiplexer (mostly used to identify logs lines)
    name: String,
}

const CHANNEL_WINDOW_SIZE: u64 = 5;

/// A channel to interact with a single client
pub struct Channel {
    /// It's identifier
    id: ChannelId,

    /// queue to send data to the endpoint through the multiplexer
    tx: mpsc::Sender<crate::proto::Message>,

    /// queue to receive message
    rx: mpsc::Receiver<crate::proto::Flow>,

    /// Stream associated
    stream: Box<dyn Stream>,

    /// Unack messages
    unack_messages: VecDeque<crate::proto::Data>,

    /// Future messages
    future_messages: VecDeque<crate::proto::Data>,

    /// Expected counter from remote
    expected_counter: u64,

    /// Current counter
    counter: u64,

    /// Last acknowledged counter
    last_ack_counter: u64,
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
    // FIXME: this methods consumes the whole buffer and returns the last message. It should
    // return the first message and not consume the rest
    match crate::proto::Message::decode(&mut cursor) {
        Ok(msg) => {
            if let Some(msg) = msg.msg {
                let position: usize = cursor
                    .position()
                    .try_into()
                    .expect("Cannot convert u64 to usize");
                // forget already read elements and shift the rest to the begin
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
            unack_messages: VecDeque::new(),
            future_messages: VecDeque::new(),
            expected_counter: 0,
            counter: 0,
            last_ack_counter: 0,
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
        self: Arc<Self>,
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
            crate::proto::message::Msg::Flow(flow) => {
                let tx = {
                    let channels = self.channels.read()?;
                    if let Some(tx) = channels.get(&flow.channel_id).map(|tx| tx.clone()) {
                        tracing::trace!("[{}] Got TX for channel {}", &self.name, &flow.channel_id);
                        Ok(tx)
                    } else {
                        tracing::warn!(
                            "[{}] Tried to write into a closed channel ({})",
                            &self.name,
                            flow.channel_id
                        );

                        Err((
                            flow.channel_id,
                            format!("Channel {} is closed", flow.channel_id),
                        ))
                    }
                };
                match tx {
                    Ok(tx) => tx.send(flow).await?,
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

    async fn dispatch_response(
        self: Arc<Self>,
        response: crate::proto::response::Response,
    ) -> Result<()> {
        let (channel_id, result) = match response {
            crate::proto::response::Response::Error(e) => (e.channel_id, Err(e.error)),
            crate::proto::response::Response::New(n) => (n.channel_id, Ok(())),
            crate::proto::response::Response::Close(c) => (c.channel_id, Ok(())),
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
                let mut channel = self.create_channel_with_id(new.channel_id, stream)?;
                self.send(crate::proto::ResponseChannelNew {
                    channel_id: new.channel_id,
                })
                .await?;
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
                let maybe_tx = {
                    let mut channels = self.channels.write()?;
                    channels.remove(&close.channel_id).map(|_| ())
                };
                if maybe_tx.is_some() {
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
    fn update_ack(&mut self, counter: u64) {
        let mut i = 0;
        while i < self.unack_messages.len() {
            if self.unack_messages[i].counter <= counter {
                self.unack_messages.remove(i);
            } else {
                i += 1;
            }
        }
        assert!(self.last_ack_counter <= counter);
        self.last_ack_counter = counter;
    }

    async fn resend_unack(&mut self) -> Result<()> {
        for message in &self.unack_messages {
            self.tx.send((self.id, message.clone()).into()).await?;
        }

        Ok(())
    }

    fn insert_future(&mut self, data: crate::proto::Data) {
        let pos = match self
            .future_messages
            .binary_search_by_key(&data.counter, |d| d.counter)
        {
            Ok(pos) => {
                if self.future_messages[pos].buffer != data.buffer {
                    tracing::error!(
                        "Got duplicate chunk with different data at {}. Ignoring.",
                        data.counter
                    );
                }
                return;
            }
            Err(pos) => pos,
        };
        self.future_messages.insert(pos, data);
    }

    async fn send_futures(&mut self) -> Result<()> {
        while self.future_messages.len() > 0 {
            if self.expected_counter == self.future_messages[0].counter {
                let data = self.future_messages.pop_front().unwrap();
                self.tx.send((self.id, data).into()).await?;
                self.expected_counter += 1;
            }
        }

        Ok(())
    }

    pub async fn pipe(&mut self) -> Result<()> {
        let mut buffer = [0u8; 8192];

        self.resend_unack().await?;
        loop {
            tokio::select! {
                res = self.stream.read(&mut buffer[..]), if self.counter - self.last_ack_counter < CHANNEL_WINDOW_SIZE => {
                    match res {
                        Ok(0) => {
                            tracing::debug!("Stream has ended");
                            break;
                        },
                        Ok(n) => {
                            let data: crate::proto::Data = (self.counter, buffer[..n].to_vec()).into();
                            self.unack_messages.push_back(data.clone());
                            self.counter += 1;
                            self.tx.send((self.id, data).into()).await?;
                        },
                        Err(e) => {
                            tracing::warn!("Got error while reading stream: {}", &e);
                            return Err(e.into());
                        }
                    }
                },
                maybe_data = self.rx.recv() => {
                    match maybe_data {
                        Some(crate::proto::Flow { flow: Some(flow), ..}) => {
                            match flow {
                                crate::proto::flow::Flow::Data(data) => {
                                    if data.counter == self.expected_counter {
                                        self.stream.write_all(&data.buffer[..]).await?;
                                        self.expected_counter += 1;
                                        self.send_futures().await?;
                                        let ack: crate::proto::Message = (self.id, self.expected_counter - 1).into();
                                        self.tx.send(ack).await?;
                                    } else {
                                        tracing::warn!("Got future packet, do not send ACK (expected: {}, received: {})", self.expected_counter, data.counter);
                                        self.insert_future(data);
                                    }
                                }
                                crate::proto::flow::Flow::Ack(ack) => {
                                    self.update_ack(ack.counter);
                                }
                            }
                        },
                        Some(crate::proto::Flow { channel_id, flow: None }) => {
                            tracing::warn!("Invalid Flow message received on channel {}", channel_id);
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

impl Drop for Channel {
    fn drop(&mut self) {
        let prog = std::env::args().next().unwrap();
        tracing::debug!("[{}] Dropping channel {}", prog, self.id);
    }
}
