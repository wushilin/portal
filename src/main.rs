pub mod quicutil;
pub mod util;
pub mod messages;
pub mod server;
pub mod client;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{net::SocketAddr};
use tokio::{net::TcpListener, task::JoinHandle};
use tracing::{info};
pub mod server_stats;
pub mod client_stats;

#[derive(Parser)]
#[command(name = "portal")]
#[command(about = "QUIC tunnel server and client")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the tunnel server
    Server {
        /// CA bundle file path
        #[arg(long, default_value = "ca.pem", help = "CA bundle file path")]
        ca_bundle: String,
        /// Server certificate file path
        #[arg(long, default_value = "server.pem", help = "server certificate file path")]
        cert: String,
        /// Server private key file path
        #[arg(long, default_value = "server.key", help = "server private key file path")]
        key: String,
        /// Bind address (e.g., 0.0.0.0)
        #[arg(long, default_value = "0.0.0.0", help = "server bind address, the tunnel server will listen on this address for QUIC connections")]
        bind_addr: String,
        /// Port number
        #[arg(long, default_value = "1741", help = "server port number, the tunnel server will listen on this port for QUIC connections")]
        port: u16,
    },
    /// Run the tunnel client
    Client {
        /// CA bundle file path
        #[arg(long, default_value = "ca.pem", help = "CA bundle file path")]
        ca_bundle: String,
        /// Client certificate file path
        #[arg(long, default_value = "client.pem", help = "client certificate file path")]
        cert: String,
        /// Client private key file path
        #[arg(long, default_value = "client.key", help = "client private key file path")]
        key: String,
        /// Server address (e.g., 127.0.0.1)
        #[arg(long, help = "tunnel server address (e.g., tunnel.abc.com)")]
        server: String,
        /// Server port number
        #[arg(long, default_value = "1741", help = "server port number, the tunnel server will listen on this port for QUIC connections")]
        port: u16,

        #[arg(short='L', help = "forward spec in [0.0.0.0]:8080@remote_host:remote_port format")]
        forward_spec: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
    .with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    )
    .init();
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|e| anyhow::anyhow!("Failed to install default crypto provider: {:?}", e))?;
    let cli = Cli::parse();

    match cli.command {
        Commands::Server {
            ca_bundle,
            cert,
            key,
            bind_addr,
            port,
        } => {
            let server_config = quicutil::build_server_config(
                &ca_bundle,
                &cert,
                &key,
                quicutil::get_server_transport_config()?,
            )?;
            
            let bind_addr: SocketAddr = format!("{}:{}", bind_addr, port).parse()?;
            let endpoint = quinn::Endpoint::server(server_config, bind_addr)?;
            info!("Server listening on {}", bind_addr);
            tokio::spawn(server::print_server_stats());
            server::run_server(endpoint).await;
            info!("Server shutdown");
        }
        Commands::Client {
            ca_bundle,
            cert,
            key,
            server,
            port,
            forward_spec,
        } => {
            let client_config = quicutil::build_client_config(
                &ca_bundle,
                &cert,
                &key,
                quicutil::get_server_transport_config()?,
            )?;
            
            let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
            endpoint.set_default_client_config(client_config);
            let server_address = format!("{}:{}", server, port);
            tokio::spawn(client::print_client_stats());
            let mut join_handles = Vec::new();
            for forward_spec in forward_spec {
                let join_handle = run_client_one_forward_spec(endpoint.clone(), server_address.clone(), forward_spec).await?;
                join_handles.push(join_handle);
            }
            for join_handle in join_handles {
                join_handle.await?;
            }
        }
    }

    Ok(())
}

pub async fn run_client_one_forward_spec(endpoint: quinn::Endpoint, server_address: String, forward_spec: String) -> Result<JoinHandle<()>> {
    let tokens = forward_spec.split('@').collect::<Vec<&str>>();
    if tokens.len() != 2 {
        return Err(anyhow::anyhow!("invalid forward spec: {}", forward_spec));
    }
    let local_bind = tokens[0];
    let remote_host = tokens[1];
    let listener = TcpListener::bind(local_bind).await?;
    info!("listening on {} forwarding to remote host: {}", local_bind, remote_host);
    let jh = tokio::spawn(client::run_client(
        listener, 
        remote_host.to_string(), 
        endpoint, server_address));
    Ok(jh)
}