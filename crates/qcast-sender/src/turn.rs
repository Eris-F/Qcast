//! Built-in TURN relay. Qcast forces ICE *relay* transport (see `host.rs` for
//! why — it sidesteps a libnice nomination crash), so a TURN server must always
//! be listening. Rather than depend on an external coturn (clean on Linux,
//! painful on Windows), Qcast runs a small TURN server in-process via the `turn`
//! crate: no extra binary, no config file, no per-machine `relay-ip` drift, and
//! it behaves identically on every platform.
//!
//! If something is already listening on the TURN port (e.g. the operator runs
//! their own coturn), we reuse it instead of binding a second time.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use turn::auth::{generate_auth_key, AuthHandler};
use turn::relay::relay_static::RelayAddressGeneratorStatic;
use turn::server::config::{ConnConfig, ServerConfig};
use turn::server::Server;
use util::vnet::net::Net;

pub const PORT: u16 = 3478;
pub const USER: &str = "qcast";
pub const PASS: &str = "qcastpass";
pub const REALM: &str = "qcast";

/// The relay backing the host. `Embedded` is our in-process server (stopped on
/// shutdown); `External` means we're reusing a relay already on the port.
pub enum Relay {
    Embedded(Server),
    External,
}

/// Long-term-credential auth for our single `qcast` user.
struct StaticAuth {
    creds: HashMap<String, Vec<u8>>,
}

impl AuthHandler for StaticAuth {
    fn auth_handle(
        &self,
        username: &str,
        _realm: &str,
        _src_addr: SocketAddr,
    ) -> Result<Vec<u8>, turn::Error> {
        self.creds
            .get(username)
            .cloned()
            .ok_or(turn::Error::ErrFakeErr)
    }
}

/// TURN is always available — it's built in. (Kept as a function so the preflight
/// check reads the same as the others.)
pub fn available() -> bool {
    true
}

/// Ensure a TURN relay is listening on `:PORT`, handing out relay candidates on
/// `lan_ip`. Reuses an external relay if the port is already taken, otherwise
/// starts the in-process server on `rt` (whose worker threads run its tasks).
pub fn ensure_running(rt: &Runtime, lan_ip: &str) -> Result<Relay> {
    if port_in_use(PORT) {
        tracing::info!("TURN relay already listening on :{PORT} — reusing it");
        return Ok(Relay::External);
    }
    let relay_ip = IpAddr::from_str(lan_ip)
        .with_context(|| format!("relay IP is not a valid address: {lan_ip}"))?;
    let server = rt
        .block_on(build(relay_ip))
        .context("start built-in TURN relay")?;
    tracing::info!(relay_ip = lan_ip, "started built-in TURN relay on :{PORT}");
    Ok(Relay::Embedded(server))
}

/// Stop the relay (no-op for an external one). Must run on the same runtime.
pub fn shutdown(rt: &Runtime, relay: Relay) {
    if let Relay::Embedded(server) = relay {
        let _ = rt.block_on(server.close());
    }
}

async fn build(relay_ip: IpAddr) -> Result<Server> {
    let conn = Arc::new(
        UdpSocket::bind(("0.0.0.0", PORT))
            .await
            .with_context(|| format!("bind TURN udp :{PORT}"))?,
    );
    let mut creds = HashMap::new();
    creds.insert(USER.to_owned(), generate_auth_key(USER, REALM, PASS));

    let server = Server::new(ServerConfig {
        conn_configs: vec![ConnConfig {
            conn,
            relay_addr_generator: Box::new(RelayAddressGeneratorStatic {
                relay_address: relay_ip,
                address: "0.0.0.0".to_owned(),
                net: Arc::new(Net::new(None)),
            }),
        }],
        realm: REALM.to_owned(),
        auth_handler: Arc::new(StaticAuth { creds }),
        channel_bind_timeout: Duration::from_secs(0),
        alloc_close_notify: None,
    })
    .await?;
    Ok(server)
}

/// True if `:port` UDP can't be bound — i.e. something already holds it.
pub(crate) fn port_in_use(port: u16) -> bool {
    StdUdpSocket::bind(("0.0.0.0", port)).is_err()
}
