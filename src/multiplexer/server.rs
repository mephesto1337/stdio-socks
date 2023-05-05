use futures::{SinkExt, StreamExt};
use std::{fmt, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::Receiver,
    time,
};
use tokio_util::codec::Decoder;

use crate::{
    proto::{self, RawCustom},
    OpenStreamFn, Result,
};

/// Multiplexer server. It will answer to requests received from stream
pub struct MultiplexerServer<C = RawCustom> {
    pub(crate) open_stream: Box<OpenStreamFn<C>>,
    pub(crate) mp: Arc<super::Multiplexer<C>>,
    pub(crate) rx: Receiver<proto::Message<C>>,
}

impl<C> MultiplexerServer<C>
where
    C: proto::Wire + fmt::Display + fmt::Debug + Send + 'static,
{
    /// Serve requests from stream
    pub async fn serve<S>(mut self, stream: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin,
    {
        let mut message_stream = proto::PacketCodec::default().framed(stream);

        let sleep_duration;
        #[cfg(feature = "heartbeat")]
        {
            sleep_duration = self.mp.config.heartbeat;
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
                        Some(Ok(msg)) => {
                            tracing::trace!("Received {msg} from stream");
                            let me = Arc::clone(&self.mp);
                            me.dispatch_message(msg, &self.open_stream).await?;
                        },
                        Some(Err(e)) => {
                            return Err(e.into());
                        }
                        None => {
                            tracing::info!("Stream has ended");
                            break;
                        }
                    }
                },
                maybe_msg = self.rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            tracing::trace!("Sending  {msg} to stream");
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
                        let msg = crate::proto::Message::<C>::Ping(self.mp.config.get_next_id());
                        tracing::trace!("Sending  {msg} to stream (heartbeat)");
                        message_stream.send(msg).await?;
                    }
                    message_stream.flush().await?;
                }
            }
        }

        Ok(())
    }
}
