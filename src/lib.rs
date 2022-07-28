pub type ChannelId = u64;

pub mod proto;

mod error;
pub use error::{Error, Result};

mod stdio;
pub use stdio::Stdio;

mod multiplexer;
pub use multiplexer::{Channel, Multiplexer, Stream};
