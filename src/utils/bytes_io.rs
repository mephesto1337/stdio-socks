use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite};

/// A read-only view of bytes
#[derive(Debug, Default)]
pub struct BytesIO {
    buffer: Vec<u8>,
    offset: usize,
}

impl From<Vec<u8>> for BytesIO {
    fn from(buffer: Vec<u8>) -> Self {
        Self { buffer, offset: 0 }
    }
}

impl From<String> for BytesIO {
    fn from(s: String) -> Self {
        Self {
            buffer: s.into_bytes(),
            offset: 0,
        }
    }
}

impl BytesIO {
    /// Creates new read-only view of bytes `data`
    pub fn new(data: impl AsRef<[u8]>) -> Self {
        Self {
            buffer: data.as_ref().to_vec(),
            offset: 0,
        }
    }
}

impl AsyncRead for BytesIO {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset < self.buffer.len() {
            let rest = &self.buffer[self.offset..];
            let n = buf.remaining().min(rest.len());
            buf.put_slice(&rest[..n]);
            self.get_mut().offset += n;
        }

        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for BytesIO {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
