# Stdio-Socks

## Description
A small tool written in Rust to add socks feature to a harden SSH server.
If the server denies dynamic forward (`AllowTcpForwarding no` or
`DisableForwarding yes`).

## TL;DR how to use

1. Setup
   ```bash
   cargo build --release
   scp target/release/server server:/tmp
   rm -f /tmp/f; mkfifo /tmp/f; cat /tmp/f | target/release/client --bind-addr=0.0.0.0:1337 | ssh server /tmp/server > /tmp/f
   ```

2. Enjoy
   ```bash
   curl -x socks5://127.0.0.1:1337 http://monip.org
   ```

## Internals
### Multiplexer
It aim to be genric. Its use Google's
[protobuf](https://developers.google.com/protocol-buffers/docs/proto3) to
(de)serialize packets. Their are 3 different types of packets:
  1. Request a new channel. Its ID is chosen by the client and a bunch of raw
     data describing the "thing" to connect to.
  2. Request to close a channel identified by its ID.
  3. Send data to an opened channel.

### Client/Server
It also used Google's
[protobuf](https://developers.google.com/protocol-buffers/docs/proto3) to
(de)serialize its data.
The client is in fact just a socks 5 server (only supporting the
[CONNECT](https://datatracker.ietf.org/doc/html/rfc1928#section-4) command).
Once a request is parsed, then its simply forwarded to the server.
