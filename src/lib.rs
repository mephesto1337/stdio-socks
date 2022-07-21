pub type ChannelId = u64;

pub(crate) mod proto {
    use std::fmt;
    include!(concat!(env!("OUT_DIR"), "/multiplexer.rs"));
    impl From<ResponseChannelNew> for Message {
        fn from(r: ResponseChannelNew) -> Self {
            Self {
                msg: Some(message::Msg::Response(Response {
                    response: Some(response::Response::New(r)),
                })),
            }
        }
    }

    impl From<ResponseChannelClose> for Message {
        fn from(r: ResponseChannelClose) -> Self {
            Self {
                msg: Some(message::Msg::Response(Response {
                    response: Some(response::Response::Close(r)),
                })),
            }
        }
    }

    impl From<(u64, u64, Vec<u8>)> for Message {
        fn from(r: (u64, u64, Vec<u8>)) -> Self {
            let (channel_id, counter, buffer) = r;
            Self {
                msg: Some(message::Msg::Data(Data {
                    channel_id,
                    counter,
                    buffer,
                })),
            }
        }
    }

    impl From<(u64, String)> for Message {
        fn from(id_error: (u64, String)) -> Self {
            let (id, error) = id_error;
            Self {
                msg: Some(message::Msg::Response(Response {
                    response: Some(response::Response::Error(Error {
                        channel_id: id,
                        error,
                    })),
                })),
            }
        }
    }

    impl From<request::Request> for Message {
        fn from(r: request::Request) -> Self {
            Message {
                msg: Some(message::Msg::Request(Request { request: Some(r) })),
            }
        }
    }

    impl From<RequestChannelClose> for Message {
        fn from(r: RequestChannelClose) -> Self {
            let req = request::Request::Close(r);
            req.into()
        }
    }

    impl From<RequestChannelNew> for Message {
        fn from(r: RequestChannelNew) -> Self {
            let req = request::Request::New(r);
            req.into()
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "Channel {}: {}", self.channel_id, self.error)
        }
    }
}

mod error;
pub use error::{Error, Result};

mod stdio;
pub use stdio::Stdio;

mod multiplexer;
pub use multiplexer::{Channel, Multiplexer, Stream};
