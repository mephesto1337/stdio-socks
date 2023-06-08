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


## Got only one reverse and want to multiplex ?

1. Upload the [server example](./examples/server.rs) on the victim system.
   ```console
   local$ gzip -9c server | base64 | tmux load-buffer -
   remote$ cat > server.64
   <TMUX PASTE>
   ^D
   base64 -d server.64 | gzip -dc > server
   chmod +x server
   ```

2. Prepare the command-line to run server-side in a file:
   ```console
   local$ cat command
   export RUST_LOG=info
   <PATH TO SERVER>/server
   ```
3. redirect your local shell (i.e. socat or nc) STDIN/STDOUT with [reredirect](https://github.com/jerome-pouiller/reredirect).
   ```console
   local$ mkfifo in out
   local$ reredirect -v -i $PWD/in -o $PWD/out $(pidof socat)
   ```

4. send the command to the remote shell
   ```console
   local$ cat command > in
   ```

5. Read (any) output from command to avoid messing up with protocol
   ```console
   local$ cat out
   ```
6. Start client
   ```console
   local$ cat out | \
   RUST_LOG=info cargo run --example=client -- --bind-addr 127.0.0.1:1337 | \
   tee /dev/null > in
   ```

7. Enjoy !


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
