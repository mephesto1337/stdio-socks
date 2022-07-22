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

    impl fmt::Display for Message {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if let Some(ref msg) = self.msg {
                fmt::Display::fmt(msg, f)
            } else {
                f.write_str("None")
            }
        }
    }

    impl fmt::Display for message::Msg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Request(ref r) => fmt::Display::fmt(r, f),
                Self::Response(ref r) => fmt::Display::fmt(r, f),
                Self::Data(ref d) => fmt::Display::fmt(d, f),
            }
        }
    }

    impl fmt::Display for Request {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if let Some(ref r) = self.request {
                fmt::Display::fmt(r, f)
            } else {
                f.write_str("Empty request")
            }
        }
    }

    impl fmt::Display for Response {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if let Some(ref r) = self.response {
                fmt::Display::fmt(r, f)
            } else {
                f.write_str("Empty response")
            }
        }
    }

    impl fmt::Display for Data {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "Data {{ channel_id: {:x}, counter: {}, buffer: {} bytes }}",
                self.channel_id,
                self.counter,
                self.buffer.len()
            )
        }
    }

    impl fmt::Display for request::Request {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::New(ref n) => {
                    write!(
                        f,
                        "Request::New {{ channel_id: {}, endpoint: ",
                        n.channel_id
                    )?;
                    for b in &n.endpoint[..] {
                        write!(f, "{:02x}", b)?;
                    }
                    f.write_str(" }}")
                }
                Self::Close(ref c) => {
                    write!(f, "Request::Close {{ channel_id: {} }}", c.channel_id)
                }
            }
        }
    }

    impl fmt::Display for response::Response {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::New(ref n) => write!(f, "Response::New {{ channel_id: {} }}", n.channel_id),
                Self::Close(ref c) => {
                    write!(f, "Response::Close {{ channel_id: {} }}", c.channel_id)
                }
                Self::Error(ref e) => write!(
                    f,
                    "Response::Error {{ channel_id: {}, error: {} }}",
                    e.channel_id, e.error
                ),
            }
        }
    }

    impl Message {
        pub fn get_data(&self) -> Option<&Data> {
            match self.msg {
                Some(message::Msg::Data(ref d)) => Some(d),
                _ => None,
            }
        }

        pub fn get_request(&self) -> Option<&request::Request> {
            match self.msg {
                Some(message::Msg::Request(Request {
                    request: Some(ref r),
                })) => Some(r),
                _ => None,
            }
        }

        pub fn get_response(&self) -> Option<&response::Response> {
            match self.msg {
                Some(message::Msg::Response(Response {
                    response: Some(ref r),
                })) => Some(r),
                _ => None,
            }
        }
    }
}

mod error;
pub use error::{Error, Result};

mod stdio;
pub use stdio::Stdio;

mod multiplexer;
pub use multiplexer::{Channel, Multiplexer, Stream};
