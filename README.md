# Portal - QUIC Tunnel Server and Client

A QUIC-based tunnel that allows you to forward TCP connections through a QUIC tunnel server.

It is like SSH Tunneling, but server allows you forward any port, as long as server ACL allows it.
The speed over long distance is almost double of typical VPN implementation. 

Client -> PORTAL CLIENT (Bind: 0.0.0.0:443, Forward to PORTAL SERVER www.google.com:443) -----QUIC-----> PORTAL SERVER -----> www.google.com:443



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

### ACL
The server can accept a `--acl aclfile.acl` argument. 

ACL example:
```
acl {
  priority = 50
  action = "allow"
  from   = ["0.0.0.0/0"]

  to {
    hosts = ["somehost.*", ".*.google.com"]
    ports = ["22", "1000-2000", "3000-4500"]
  }
}
```

From is CIDR list. It is evaluated as the real remote client (the client of portal client) ip address.

action can be "allow" or "deny"

Priority decides rule execution order. higher priority rules will be evaluated first. 
For same priority rule, the rule defined earlier in the file will be evaluated first. Note priority is 32 bit signed integer. You can put negative priority if you want.

To means a list of host by regex, case insensitive and a list of port, or port ranges.

The ACL, if defined, will be evaluated in high priority -> low priority, earlier -> later for same priority until a decision is made. 

If no rule matches, default is deny when no rule is matched. But you can change behavior by defining a rule with priority of -999999999, that accepts every thing.

Note ACL matches the real client's IP and target address!

A rejected client will be disconnected.

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

## About status

The portal prints stats like this periodcally.

```
2025-11-15T08:05:35.836529Z  INFO portal::server: Stats: TC=5,AC=4,TS=213,AS=2,TUC=213,AUC=2,SENT=5.46 GiB,RECV=267.22 MiB
```

- TC = Total Connections (QUIC)
- AC = Active Connections (QUIC)
- TS = Total number of Streams (over QUIC)
- AS = Active number of Streams (over QUIC)
- TUC = Total Upstream Connections (The bridged TCP streams)
- AUC = Active Upstream Connections (Active bridged TCP Streams, typically = AS)
- SENT = Total data sent to remote server (QUIC clients)
- RECV = Total data received from remote server (QUIC clients)



## Security Note

This implementation uses self-signed certificates and disables certificate verification for testing purposes. **Do not use in production without proper certificate management and verification.**



