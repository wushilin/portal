pub mod quicutil;
pub mod util;
pub mod messages;
pub mod server;
pub mod client;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{net::SocketAddr};
use tokio::net::{TcpListener};
use tokio_tree_context::Context;
use tracing::{info};


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
        #[arg(long)]
        ca_bundle: String,
        /// Server certificate file path
        #[arg(long)]
        cert: String,
        /// Server private key file path
        #[arg(long)]
        key: String,
        /// Bind address (e.g., 0.0.0.0)
        #[arg(long, default_value = "0.0.0.0")]
        bind_addr: String,
        /// Port number
        #[arg(long, default_value = "1741")]
        port: u16,
    },
    /// Run the tunnel client
    Client {
        /// CA bundle file path
        #[arg(long)]
        ca_bundle: String,
        /// Client certificate file path
        #[arg(long)]
        cert: String,
        /// Client private key file path
        #[arg(long)]
        key: String,
        /// Server address (e.g., 127.0.0.1)
        #[arg(long)]
        server: String,
        /// Server port number
        #[arg(long, default_value = "1741")]
        port: u16,

        #[arg(long, default_value = "0.0.0.0")]
        local_bind:String,

        #[arg(long)]
        local_port: u16,

        #[arg(long)]
        target_address: String,
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
            let context = Context::new();
            info!("Server listening on {}", bind_addr);
            server::run_server(context, endpoint).await;
            info!("Server shutdown");
        }
        Commands::Client {
            ca_bundle,
            cert,
            key,
            server,
            port,
            local_bind,
            local_port,
            target_address,
        } => {
            let client_config = quicutil::build_client_config(
                &ca_bundle,
                &cert,
                &key,
                quicutil::get_server_transport_config()?,
            )?;
            
            let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
            endpoint.set_default_client_config(client_config);
            let context = Context::new();
            let tcp_listener = TcpListener::bind((local_bind.clone(), local_port)).await?;
            info!("client listening on {}:{}", local_bind, local_port);
            client::run_client(context, tcp_listener, target_address, endpoint, server, port).await;
        }
    }

    Ok(())
}
