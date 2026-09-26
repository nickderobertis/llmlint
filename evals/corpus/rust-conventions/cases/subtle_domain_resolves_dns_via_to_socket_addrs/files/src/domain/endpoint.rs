use std::net::{SocketAddr, ToSocketAddrs};

/// A configured upstream: host name and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
}

/// Parse `host:port`, defaulting the port to 443 when it is omitted.
pub fn parse_upstream(spec: &str) -> Option<Upstream> {
    let (host, port) = match spec.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (spec, 443),
    };
    if host.is_empty() {
        return None;
    }
    Some(Upstream { host: host.to_string(), port })
}

/// Whether two upstreams name the same host (ignoring ASCII case) and port.
pub fn same_upstream(left: &Upstream, right: &Upstream) -> bool {
    left.host.eq_ignore_ascii_case(&right.host) && left.port == right.port
}

/// The socket addresses `upstream` refers to, in resolver order; empty when it
/// does not resolve.
pub fn socket_addrs(upstream: &Upstream) -> Vec<SocketAddr> {
    (upstream.host.as_str(), upstream.port)
        .to_socket_addrs()
        .map(|addrs| addrs.collect())
        .unwrap_or_default()
}
