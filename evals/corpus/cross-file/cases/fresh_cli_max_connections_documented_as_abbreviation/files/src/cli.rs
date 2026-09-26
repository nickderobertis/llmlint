use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

/// pgproxy — a tiny connection-pooling proxy for PostgreSQL.
#[derive(Debug, Parser)]
#[command(name = "pgproxy", version)]
pub struct Opts {
    /// Address to accept client connections on.
    #[arg(long = "listen", default_value = "127.0.0.1:6432")]
    pub listen: SocketAddr,

    /// Upstream PostgreSQL server.
    #[arg(long = "upstream", default_value = "127.0.0.1:5432")]
    pub upstream: SocketAddr,

    /// Maximum number of pooled server connections.
    #[arg(long = "max-connections", default_value_t = 20)]
    pub max_connections: u32,

    /// Close idle server connections after this many seconds.
    #[arg(long = "idle-timeout", default_value_t = 300)]
    pub idle_timeout: u64,

    /// PEM certificate for TLS towards clients.
    #[arg(long = "tls-cert")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key for TLS towards clients.
    #[arg(long = "tls-key", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,

    /// Log each query's duration.
    #[arg(long = "log-queries")]
    pub log_queries: bool,
}

impl Opts {
    pub fn tls_enabled(&self) -> bool {
        self.tls_cert.is_some() && self.tls_key.is_some()
    }
}
