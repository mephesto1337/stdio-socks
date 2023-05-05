//! Different structure to pass data as if they were [`Stream`]

mod stdio;
pub use stdio::Stdio;

mod empty;
pub use empty::DevNull;

mod bytes_io;
pub use bytes_io::BytesIO;
