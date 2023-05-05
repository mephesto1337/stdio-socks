use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite};

/// Wraps STDIN/STDOUT
pub struct Stdio {
    stdin: tokio::io::Stdin,
    stdout: tokio::io::Stdout,
}

impl Default for Stdio {
    fn default() -> Self {
        Self::new()
    }
}

impl Stdio {
    /// Create a new Stdio stream
    pub fn new() -> Self {
        Self {
            stdin: tokio::io::stdin(),
            stdout: tokio::io::stdout(),
        }
    }
}

impl AsyncRead for Stdio {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let stdin = Pin::new(&mut self.stdin);
        AsyncRead::poll_read(stdin, cx, buf)
    }
}

impl AsyncWrite for Stdio {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let stdout = Pin::new(&mut self.stdout);
        AsyncWrite::poll_write(stdout, cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let stdout = Pin::new(&mut self.stdout);
        AsyncWrite::poll_flush(stdout, cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let stdout = Pin::new(&mut self.stdout);
        AsyncWrite::poll_shutdown(stdout, cx)
    }
}
