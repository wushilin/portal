# Portal - QUIC Tunnel Server and Client

A QUIC-based tunnel that allows you to forward TCP connections through a QUIC tunnel server.

## Features

- **Tunnel Server**: Accepts QUIC connections and forwards traffic to target hosts
- **Tunnel Client**: Listens on a local TCP port and tunnels connections through QUIC to a remote server
- **Bidirectional Streaming**: Full bidirectional data forwarding between client and server
- **Length-prefixed Protocol**: Uses length-prefixed messages for target address communication

## Building

```bash
cargo build --release
```

## Usage

### Server Mode

Start the tunnel server:

```bash
./target/release/portal server --listen 0.0.0.0:4433
```

The server will:
- Listen for QUIC connections on the specified address
- Accept bidirectional streams from clients
- Read length-prefixed target addresses (host:port format)
- Connect to the target and forward data bidirectionally

### Client Mode

Start the tunnel client:

```bash
./target/release/portal client \
    --local 0.0.0.0:1122 \
    --server 127.0.0.1:4433 \
    --target example.com:80
```

The client will:
- Listen on the local address (e.g., `0.0.0.0:1122`)
- When a connection is made to the local address, it:
  1. Opens a QUIC connection to the tunnel server
  2. Opens a bidirectional stream
  3. Sends the target address (length-prefixed)
  4. Waits for server confirmation
  5. Starts bidirectional data forwarding

## Example

1. Start the server:
   ```bash
   ./target/release/portal server --listen 0.0.0.0:4433
   ```

2. Start the client:
   ```bash
   ./target/release/portal client \
       --local 0.0.0.0:8080 \
       --server 127.0.0.1:4433 \
       --target httpbin.org:80
   ```

3. Connect to the local port:
   ```bash
   curl http://localhost:8080/get
   ```

The connection will be tunneled through QUIC to the server, which will then connect to `httpbin.org:80`.

## Protocol Details

- **Target Address Format**: Length-prefixed UTF-8 string in `host:port` format
  - First 4 bytes: Big-endian u32 length
  - Remaining bytes: UTF-8 string (e.g., "example.com:80")
- **Server Confirmation**: 1-byte ACK (value: 1) sent after successful target connection
- **Data Forwarding**: Bidirectional streaming of raw bytes after confirmation

## Security Note

This implementation uses self-signed certificates and disables certificate verification for testing purposes. **Do not use in production without proper certificate management and verification.**



