use futures::{Sink, Stream};
use nom::{
    combinator::{map, map_opt},
    error::context,
    multi::length_data,
    number::streaming::{be_u32, be_u8},
    Offset,
};
use pin_project::pin_project;
use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::ChannelId;

pub trait Wire: Sized {
    /// Encodes it-self into a buffer
    fn encode(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        self.encode_into(&mut buffer);
        buffer
    }

    /// Encodes is-self into specified buffer
    fn encode_into(&self, buffer: &mut Vec<u8>);

    /// Decodes it-self from wire
    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>;

    fn decode_verbose(buffer: &[u8]) -> nom::IResult<&[u8], Self, nom::error::VerboseError<&[u8]>> {
        Self::decode(buffer)
    }
}

impl Wire for String {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let data = self.as_bytes();
        let data_len: u32 = data.len().try_into().expect("Name too long");
        buffer.extend_from_slice(&data_len.to_be_bytes()[..]);
        buffer.extend_from_slice(data);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context(
            "String",
            map_opt(length_data(be_u32), |bytes: &[u8]| {
                std::str::from_utf8(bytes).ok().map(|s| s.to_owned())
            }),
        )(buffer)
    }
}

impl Wire for Vec<u8> {
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        let size: u32 = self
            .len()
            .try_into()
            .expect("Vec's length does not fit into u32");
        buffer.extend_from_slice(&size.to_be_bytes()[..]);
        buffer.extend_from_slice(&self[..]);
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        context("Vec", map(length_data(be_u32), |data: &[u8]| data.to_vec()))(buffer)
    }
}

impl<T> Wire for Option<T>
where
    T: Wire,
{
    fn encode_into(&self, buffer: &mut Vec<u8>) {
        if let Some(value) = self.as_ref() {
            buffer.push(1);
            value.encode_into(buffer);
        } else {
            buffer.push(0);
        }
    }

    fn decode<'i, E>(buffer: &'i [u8]) -> nom::IResult<&'i [u8], Self, E>
    where
        E: nom::error::ParseError<&'i [u8]> + nom::error::ContextError<&'i [u8]>,
    {
        let (rest, has_value) = context("Optional value", be_u8)(buffer)?;
        match has_value {
            0 => Ok((rest, None)),
            1 => map(T::decode, Some)(rest),
            _ => Err(nom::Err::Failure(E::add_context(
                buffer,
                "Invalid value for optional",
                nom::error::make_error(buffer, nom::error::ErrorKind::NoneOf),
            ))),
        }
    }
}

mod address;
mod endpoint;
mod message;
mod request;
mod response;

pub use address::Address;
pub use endpoint::{Endpoint, RawCustom};
pub use message::Message;
pub use request::Request;
pub use response::Response;

impl<C> From<Response<C>> for Message<C> {
    fn from(r: Response<C>) -> Self {
        Self::Response(r)
    }
}

impl<C> From<Request<C>> for Message<C> {
    fn from(r: Request<C>) -> Self {
        Self::Request(r)
    }
}

impl<C> From<(ChannelId, Vec<u8>)> for Message<C> {
    fn from(r: (ChannelId, Vec<u8>)) -> Self {
        let (channel_id, data) = r;
        Self::Data { channel_id, data }
    }
}

impl<C> From<(ChannelId, String)> for Message<C> {
    fn from(r: (ChannelId, String)) -> Self {
        let (channel_id, message) = r;
        Self::Response(Response::Error {
            channel_id,
            message,
        })
    }
}

/// A struct that implements [`futures::Stream`] and [`futures::Sink`] for Item = [`Message`]
/// It internally uses a [`tokio::io::BufStream`]
#[pin_project]
pub struct MessageStream<C, S> {
    #[pin]
    inner: S,
    tx_buffer: Vec<u8>,
    tx_offset: usize,
    rx_buffer: Vec<u8>,
    rx_offset: usize,
    _type: PhantomData<C>,
}

impl<C, S> MessageStream<C, S>
where
    S: AsyncRead + AsyncWrite,
{
    pub fn new(stream: S) -> Self {
        Self {
            inner: stream,
            rx_buffer: Vec::new(),
            tx_offset: 0,
            tx_buffer: Vec::new(),
            rx_offset: 0,
            _type: PhantomData,
        }
    }
}

enum MessageStreamResult<C> {
    Ok(Message<C>),
    Incomplete(nom::Needed),
    Err(nom::error::VerboseError<Vec<u8>>),
}

impl<C, S> MessageStream<C, S>
where
    C: Wire + std::fmt::Display,
{
    fn move_to_start(buffer: &mut Vec<u8>, offset: &mut usize) {
        if *offset < 2048 {
            return;
        }

        let destination = buffer.as_mut_ptr();
        let size = buffer[*offset..].len();
        let source = buffer[*offset..].as_ptr();
        *offset = 0;

        unsafe {
            std::intrinsics::copy(source, destination, size);
            buffer.set_len(size);
        }
    }

    fn get_buffered_next(buffer: &mut Vec<u8>, offset: &mut usize) -> MessageStreamResult<C> {
        match Message::<C>::decode_verbose(&buffer[*offset..]) {
            Ok((rest, msg)) => {
                let size = buffer[*offset..].offset(rest);
                tracing::trace!("Received {size} bytes message: {msg}");
                *offset += size;
                Self::move_to_start(buffer, offset);
                MessageStreamResult::Ok(msg)
            }
            Err(ne) => match crate::error::nom_to_owned(ne) {
                nom::Err::Incomplete(n) => MessageStreamResult::Incomplete(n),
                nom::Err::Error(e) => MessageStreamResult::Err(e),
                nom::Err::Failure(e) => MessageStreamResult::Err(e),
            },
        }
    }
}

impl<C, S> Stream for MessageStream<C, S>
where
    C: Wire + std::fmt::Display,
    S: AsyncRead,
{
    type Item = Message<C>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut project = self.project();
        loop {
            tracing::trace!(
                "rx_buffer: len={len}, capacity={cap}, offset={offset}",
                len = project.rx_buffer.len(),
                cap = project.rx_buffer.capacity(),
                offset = project.rx_offset
            );
            match Self::get_buffered_next(project.rx_buffer, project.rx_offset) {
                MessageStreamResult::Ok(m) => return Poll::Ready(Some(m)),
                MessageStreamResult::Incomplete(n) => {
                    project.rx_buffer.reserve(
                        match n {
                            nom::Needed::Unknown => 0,
                            nom::Needed::Size(s) => s.get(),
                        }
                        .max(1024),
                    );
                    let mut read_buf = ReadBuf::uninit(project.rx_buffer.spare_capacity_mut());
                    if let Err(e) = ready!(project.inner.as_mut().poll_read(cx, &mut read_buf)) {
                        tracing::error!("Got underlying IO error: {e}");
                        return Poll::Ready(None);
                    }
                    let n = read_buf.filled().len();
                    tracing::trace!("Read {n} bytes from underlying stream");
                    if n == 0 {
                        // EOF
                        return Poll::Ready(None);
                    } else {
                        let new_len = project.rx_buffer.len() + n;
                        unsafe { project.rx_buffer.set_len(new_len) };

                        // next loop, rety to parse
                    }
                }
                MessageStreamResult::Err(e) => {
                    tracing::error!("Got parsing error: {e:?}");
                    return Poll::Ready(None);
                }
            }
        }
    }
}

impl<C, S> Sink<Message<C>> for MessageStream<C, S>
where
    C: Wire + std::fmt::Display,
    S: AsyncWrite,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut project = self.project();
        loop {
            tracing::trace!(
                "tx_buffer: len={len}, capacity={cap}, offset={offset}",
                len = project.tx_buffer.len(),
                cap = project.tx_buffer.capacity(),
                offset = project.tx_offset
            );
            let buf = &project.tx_buffer[*project.tx_offset..];
            if buf.is_empty() {
                return Poll::Ready(Ok(()));
            }
            match ready!(project.inner.as_mut().poll_write(cx, buf)) {
                Ok(n) => {
                    tracing::trace!("Wrote {n} to underlying stream");
                    *project.tx_offset += n;
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<C>) -> Result<(), Self::Error> {
        let project = self.project();
        tracing::trace!("Will send {item} to stream");
        item.encode_into(project.tx_buffer);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_shutdown(cx)
    }
}
