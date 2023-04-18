pub type ChannelId = u64;

pub mod proto;

mod error;
pub use error::{Error, Result};

mod stdio;
pub use stdio::Stdio;

mod empty;
pub use empty::DevNull;

mod bytes_io;
pub use bytes_io::BytesIO;

mod multiplexer;
pub use multiplexer::{Channel, Multiplexer, OpenStreamResult, Stream};
