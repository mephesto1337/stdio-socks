use std::{
    fmt, io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    proto::{self, RawCustom, Wire},
    Channel, ChannelId, Result,
};

/// Multiplexer client
pub struct MultiplexerClient<C = RawCustom> {
    pub(crate) mp: Arc<super::Multiplexer<C>>,
    pub(crate) channel_id: AtomicU64,
}

impl<C> MultiplexerClient<C>
where
    // C: proto::Wire + Send + 'static,
    C: Wire + fmt::Display + fmt::Debug + Send + 'static,
{
    fn get_new_multiplexer(&self) -> Arc<super::Multiplexer<C>> {
        Arc::clone(&self.mp)
    }

    /// Request remote end to open a channel with the given endpoint. Returns the id chosen for
    /// both ends.
    pub async fn request_open(&self, endpoint: proto::Endpoint<C>) -> Result<Channel<C, ()>> {
        let channel_id = self.channel_id.fetch_add(1, Ordering::Relaxed);
        let channel = self.mp.create_channel_with_id(channel_id, ())?;
        let rx = self
            .get_new_multiplexer()
            .request_open(channel_id, endpoint)
            .await?;

        match rx
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, format!("{e}")))?
        {
            proto::Response::Open { channel_id } => {
                tracing::debug!("Remote opened channel#{channel_id}");
                Ok(channel)
            }
            proto::Response::Error {
                channel_id,
                message,
            } => {
                tracing::error!("Remote could not open channel#{channel_id}: {message}");
                Err(io::Error::new(io::ErrorKind::Other, message).into())
            }
        }
    }

    /// Creates locally a channel associated with stream. You should call the method after having
    /// requesting an remote opening:
    ///
    /// ```rust
    /// let client: MultiplexerClient = ...;
    /// let stream: TcpStream = ...;
    /// let channel_id = client.request_open(Endpoint { ... }).await.unwrap();
    /// let mut channel = client.create_channel(stream, channel_id).unwrap();
    /// channel.pipe().await.unwrap();
    /// ```
    pub fn create_channel<S>(&self, channel_id: ChannelId) -> Result<Channel<C, ()>>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let channel = self.mp.create_channel_with_id(channel_id, ())?;
        Ok(channel)
    }
}
