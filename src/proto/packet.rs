use bytes::{Buf, BytesMut};
use std::{fmt, io, marker::PhantomData, mem::size_of};
use tokio_util::codec::{Decoder, Encoder};

use super::Wire;

/// Messages that can be exchanged
#[derive(Debug, Clone, PartialEq, Eq)]
struct Packet {
    buffer: Vec<u8>,
    offset: usize,
}

#[cfg(debug_assertions)]
const PACKET_TAG: &[u8; 8] = b"multiplx";

impl Packet {
    fn clear(&mut self) {
        self.buffer.clear();
        self.offset = 0;
    }

    fn set<T: Wire>(&mut self, value: &T) {
        self.clear();
        self.push(value);
    }

    fn push<T: Wire>(&mut self, value: &T) {
        #[cfg(debug_assertions)]
        {
            self.buffer.extend_from_slice(&PACKET_TAG[..]);
        }

        let size_offset = self.buffer.len();
        self.buffer.extend_from_slice(&[0u8; size_of::<u32>()][..]);

        let old_size = self.buffer.len();
        value.encode_into(&mut self.buffer);
        let new_size = self.buffer.len();

        let packet_size: u32 = (new_size - old_size).try_into().unwrap();
        self.buffer[size_offset..][..size_of::<u32>()]
            .copy_from_slice(&packet_size.to_be_bytes()[..]);
    }
}

impl fmt::Display for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Packet({} bytes)", self.buffer.len())
    }
}

pub struct PacketCodec<M> {
    packet: Packet,
    _marker: PhantomData<M>,
}

impl<M> PacketCodec<M> {
    pub fn new() -> Self {
        Self {
            packet: Packet {
                buffer: Vec::new(),
                offset: 0,
            },
            _marker: PhantomData,
        }
    }
}
impl<M> Decoder for PacketCodec<M>
where
    M: Wire,
{
    type Item = M;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut offset = 0;
        #[cfg(debug_assertions)]
        {
            if src.len() < PACKET_TAG.len() {
                src.reserve(PACKET_TAG.len() - src.len());
                return Ok(None);
            }
            if !src.starts_with(&PACKET_TAG[..]) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Packet does not start with magic",
                ));
            }
            offset += PACKET_TAG.len();
        }

        if src.len() < offset + size_of::<u32>() {
            src.reserve(offset + size_of::<u32>() - src.len());
            return Ok(None);
        }
        let mut raw_packet_size = [0u8; size_of::<u32>()];
        raw_packet_size.copy_from_slice(&src[offset..][..size_of::<u32>()][..]);
        let packet_size = u32::from_be_bytes(raw_packet_size) as usize;
        offset += size_of::<u32>();

        if src.len() < offset + packet_size {
            src.reserve(offset + packet_size - src.len());
            return Ok(None);
        }

        let input = &src[offset..][..packet_size];
        match M::decode_verbose(input) {
            Ok((_, value)) => {
                src.advance(offset + packet_size);
                Ok(Some(value))
            }
            Err(ne) => match crate::error::nom_to_owned(ne) {
                nom::Err::Incomplete(_) => unreachable!("Bad serialization"),
                nom::Err::Error(e) | nom::Err::Failure(e) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{e:#?}"),
                )),
            },
        }
    }
}

impl<M> Encoder<M> for PacketCodec<M>
where
    M: Wire,
{
    type Error = io::Error;

    fn encode(&mut self, item: M, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.packet.set(&item);
        dst.extend_from_slice(&self.packet.buffer[..]);
        Ok(())
    }
}
