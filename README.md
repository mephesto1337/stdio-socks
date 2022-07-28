# Stdio-Socks

## Description
A small tool written in Rust to add socks feature to a harden SSH server.
If the server denies dynamic forward (`AllowTcpForwarding no` or
`DisableForwarding yes`).

## TL;DR how to use

1. Setup
   ```bash
   cargo build --release --examples
   scp target/release/examples/server server:/tmp
   export RUST_LOG=info
   rm -f /tmp/f; mkfifo /tmp/f; cat /tmp/f | target/release/examples/client --bind-addr=0.0.0.0:1337 | ssh server /tmp/server > /tmp/f
   ```

2. Enjoy
   ```bash
   curl -x socks5://127.0.0.1:1337 http://monip.org
   ssh -o ProxyCommand='nc -X 5 -x 127.0.0.1:1337 %h %p' $INTERNAL_IP
   ```

## Internals
### Event loop
It uses [tokio](https://tokio.rs/) and is fully asynchronous to avoir spawing
too many threads while handling each connection.

### Multiplexer
It aim to be generic. Its use a custom (simle) protocol and the parsing is done
thanks to [nom](https://github.com/Geal/nom). There are 3 different types of
packets:
  1. Request a new channel. Its ID is chosen by the client and the remote
     endpoint. For now a hostname/port or a path to an UNIX socket.
     data describing the "thing" to connect to.
  2. Request to close a channel identified by its ID.
  3. Send data to an opened channel.

### Client/Server
Theses examples are just to demontrate the multuiplexer capabilities. But you
can do anything else you want with it.
The client is in fact just a socks 5 server (only supporting the
[CONNECT](https://datatracker.ietf.org/doc/html/rfc1928#section-4) command).
Once a request is parsed, then its simply forwarded to the server.
Also, the client refuse to open new stream (to avoid the hacker being him-self
hacked).
