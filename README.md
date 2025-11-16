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
./target/release/portal server
```

or
```bash
./target/release/portal server --help
Usage: portal server [OPTIONS]

Options:
      --ca-bundle <CA_BUNDLE>  CA bundle file path [default: ca.pem]
      --cert <CERT>            server certificate file path [default: server.pem]
      --key <KEY>              server private key file path [default: server.key]
      --bind-addr <BIND_ADDR>  server bind address, the tunnel server will listen on this address for QUIC connections [default: 0.0.0.0]
      --port <PORT>            server port number, the tunnel server will listen on this port for QUIC connections [default: 1741]
  -i <STATS_INTERVAL>          stats interval in seconds. default 60 seconds [default: 60]
      --acl <ACL>              ACL file path, if not provided, no ACL will be used. See acl.hcl for format
  -h, --help                   Print help
```

The server will:
- Listen for QUIC connections on the specified address
- Accept bidirectional streams from clients
- Read client connect request like a proxy
- Connect to the target and forward data bidirectionally

### Client Mode

Start the tunnel client:

Forward localhost of client machine 1022 to server's ssh port at localhost.
```bash
./target/release/portal client --server myhost.acme.com -L "0.0.0.0:1022@localhost:22"
```

The client will:
- Listen on the local address (e.g., `0.0.0.0:1022`)
- When a connection is made to the local address, it:
  1. Opens a QUIC connection to the tunnel server
  2. Opens a bidirectional stream
  3. Sends the target address (length-prefixed)
  4. Waits for server confirmation
  5. Starts bidirectional data forwarding

Or
```bash
./target/release/portal client --help
Run the tunnel client

Usage: portal client [OPTIONS] --server <SERVER>

Options:
      --ca-bundle <CA_BUNDLE>  CA bundle file path [default: ca.pem]
      --cert <CERT>            client certificate file path [default: client.pem]
      --key <KEY>              client private key file path [default: client.key]
      --server <SERVER>        tunnel server address (e.g., tunnel.abc.com)
      --port <PORT>            server port number, the tunnel server will listen on this port for QUIC connections [default: 1741]
  -L <FORWARD_SPEC>            forward spec in 0.0.0.0:8080@remote_host:remote_port format. First part before @ is the local bind. Second part after @ is the remote address to forward to
  -i <STATS_INTERVAL>          stats interval in seconds. default no stats [default: 60]
  -h, --help                   Print help
```



## Protocol Details

- **Target Address Format**: Must be hostname:port format. Support bare IPv4/IPv6 + port format too.
- **Client send request, server respond**: Like kafka, it is request/response system.
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

## Certificate limit
Default certs is valid for *.wushilin.net, *.acme.com and *.test.com.

Note that QUINN reject *.local SAN, so if you use such certs, it won't work. try other domain suffix, .local suffix is not working with Quinn!



