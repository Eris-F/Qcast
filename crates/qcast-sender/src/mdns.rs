//! mDNS publish + browse for LAN session discovery.
//!
//! The **sender** publishes a `_qcast._tcp.local.` service whose TXT records
//! carry the session's pairing code (= `producer-peer-id` on webrtcsink),
//! schema version, app name, and build version. The **receiver** browses the
//! same service type and surfaces a list of nearby sessions, so the user can
//! one-click join without typing the code (cross-LAN still uses the typed-code
//! fallback). See `deploy/UI_REWRITE.md` §5–§6.
//!
//! This module is intentionally UI-agnostic: it exposes the publish handle
//! ([`MdnsPublisher`]) and a polling-style browser snapshot ([`MdnsBrowser`])
//! so both the Tauri commands layer and integration tests can drive it
//! without an event loop. The publisher's `Drop` unregisters cleanly so a
//! stop-share leaves no stale entry on the network.
//!
//! Crate choice: `mdns-sd` is pure-Rust (no avahi/bonjour) so it works the
//! same way on Linux and Windows — important for the Win↔Win pivot.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The Bonjour-style service type all Qcast sessions register under.
///
/// Per RFC 6763 §4.1 this must be `_<name>._tcp.local.` (note the leading and
/// trailing dots that `mdns-sd` requires).
pub const SERVICE_TYPE: &str = "_qcast._tcp.local.";

/// TXT-record schema version. We bump this if/when the meaning of any TXT key
/// changes incompatibly; receivers should fail-closed on a mismatched version
/// rather than guess at unknown fields.
pub const SCHEMA_VERSION: &str = "1";

/// The well-known TXT keys advertised by every Qcast publisher. Defined as
/// constants so both publish and parse use the exact same spelling.
pub const TXT_KEY_VERSION: &str = "v";
pub const TXT_KEY_PEER_ID: &str = "peer-id";
pub const TXT_KEY_APP: &str = "app";
pub const TXT_KEY_BUILD: &str = "build";

/// Constant TXT value for `app=` — pinned so a future rename of the binary
/// can't accidentally orphan old receivers.
pub const TXT_APP_VALUE: &str = "qcast";

/// A LAN-discovered Qcast session, snapshot-style.
///
/// The browser keeps these keyed by `peer_id` and updates `last_seen` on every
/// re-resolution; receivers can use `last_seen` to age-out entries the user
/// hasn't picked yet if mDNS removal events are lost.
#[derive(Debug, Clone)]
pub struct LanSession {
    /// The pairing code (= `producer-peer-id` on the sender's webrtcsink).
    pub peer_id: String,
    /// Human-readable hostname the sender advertised (`ERIS-DESKTOP`).
    pub display_name: String,
    /// First resolved IP + the advertised port (`192.168.1.50:8443`). The
    /// receiver dials this to talk to the signalling server.
    pub addr: String,
    /// Wall-clock instant of the most recent resolve. Used to age-out entries
    /// if a `ServiceRemoved` event is missed.
    pub last_seen: Instant,
}

/// Publishes a single `_qcast._tcp.local.` service for the lifetime of the
/// handle. Dropping it (or going out of scope) cleanly unregisters.
///
/// `peer_id` is the access code (e.g. `GHF/ABA/6TJ`); see `access_code.rs`
/// for generation. `hostname` is the local computer name (`hostname` crate
/// or a stable fallback). `signalling_port` is the port webrtcsink's
/// signalling server is bound to.
pub struct MdnsPublisher {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsPublisher {
    /// Register the service. Returns once the registration request is queued
    /// (`mdns-sd` announces asynchronously on its own thread). On success the
    /// service is visible to LAN browsers within ~1 multicast round-trip.
    pub fn publish(peer_id: &str, hostname: &str, signalling_port: u16) -> Result<Self> {
        // ServiceInfo wants a "host" in the DNS sense (must end in `.local.`).
        // Sanitize the OS-reported hostname so a stray dot or trailing space
        // can't make ServiceInfo::new reject the registration.
        let host_dns = format!("{}.local.", sanitize_hostname(hostname));

        let properties: HashMap<String, String> = [
            (TXT_KEY_VERSION.to_string(), SCHEMA_VERSION.to_string()),
            (TXT_KEY_PEER_ID.to_string(), peer_id.to_string()),
            (TXT_KEY_APP.to_string(), TXT_APP_VALUE.to_string()),
            (TXT_KEY_BUILD.to_string(), env!("CARGO_PKG_VERSION").to_string()),
        ]
        .into_iter()
        .collect();

        let daemon = ServiceDaemon::new().context("create mdns-sd ServiceDaemon")?;

        // `enable_addr_auto()` lets the daemon fill in the host's current
        // IPv4/IPv6 addresses (and re-publish on IP changes) so we don't have
        // to plumb the LAN IP in here separately.
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            // Instance name — the user-visible label in the receiver's list.
            hostname,
            &host_dns,
            "",
            signalling_port,
            properties,
        )
        .context("build mdns ServiceInfo")?
        .enable_addr_auto();

        let fullname = info.get_fullname().to_string();

        daemon.register(info).context("register mdns service")?;
        tracing::info!(
            service = %fullname,
            peer_id = %peer_id,
            port = signalling_port,
            "mDNS service published"
        );
        Ok(Self { daemon, fullname })
    }

    /// The fully-qualified DNS name the service was registered under
    /// (`hostname._qcast._tcp.local.`). Useful in tests and logs.
    pub fn fullname(&self) -> &str {
        &self.fullname
    }
}

impl Drop for MdnsPublisher {
    fn drop(&mut self) {
        // Best-effort unregister + daemon shutdown. We don't block on the
        // returned receivers because (a) Drop can't be async and (b) we don't
        // want a slow LAN to slow process exit; the daemon's own thread will
        // send the goodbye packet before exiting.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
        tracing::debug!(service = %self.fullname, "mDNS service unpublished");
    }
}

/// Background-thread mDNS browser for the `_qcast._tcp.local.` service.
///
/// `start()` spawns a worker that consumes `ServiceResolved` / `ServiceRemoved`
/// events into a shared `HashMap<peer_id, LanSession>`. UIs can either poll
/// [`MdnsBrowser::snapshot`] on a 1.5s tick (the design doc target) or wire
/// in their own event source later — this API stays poll-only so the same
/// code drives both unit tests and the Tauri commands layer.
pub struct MdnsBrowser {
    daemon: ServiceDaemon,
    sessions: Arc<Mutex<HashMap<String, LanSession>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MdnsBrowser {
    /// Begin browsing. The internal worker thread runs until the browser is
    /// dropped (which shuts the daemon down; the worker's receiver then
    /// disconnects and the thread exits).
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("create mdns-sd ServiceDaemon for browse")?;
        let rx = daemon.browse(SERVICE_TYPE).context("start mdns browse")?;
        let sessions: Arc<Mutex<HashMap<String, LanSession>>> = Arc::new(Mutex::new(HashMap::new()));

        let sessions_for_worker = sessions.clone();
        let worker = std::thread::Builder::new()
            .name("qcast-mdns-browse".into())
            .spawn(move || {
                // recv() blocks until the next event; it errors when the
                // daemon shuts down (our cue to exit cleanly).
                while let Ok(event) = rx.recv() {
                    handle_event(&event, &sessions_for_worker);
                }
            })
            .context("spawn mdns browse worker")?;

        Ok(Self {
            daemon,
            sessions,
            worker: Some(worker),
        })
    }

    /// Snapshot of the currently-known LAN sessions. Cheap (one lock + clone)
    /// so it's safe to call on a 1.5s UI tick.
    pub fn snapshot(&self) -> Vec<LanSession> {
        match self.sessions.lock() {
            Ok(guard) => guard.values().cloned().collect(),
            Err(poisoned) => {
                // A worker-side panic shouldn't take the whole UI down; the
                // map is just a cache, recovering it is safe.
                poisoned.into_inner().values().cloned().collect()
            }
        }
    }
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
        if let Some(h) = self.worker.take() {
            // Don't block forever if the worker is stuck on recv; the
            // shutdown above should let it exit promptly.
            let _ = h.join();
        }
    }
}

/// Decode one mDNS event into an update on the shared session map. Pulled out
/// so the unit tests can drive it without standing up a real network.
fn handle_event(event: &ServiceEvent, sessions: &Arc<Mutex<HashMap<String, LanSession>>>) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let Some(peer_id) = info
                .get_properties()
                .get_property_val_str(TXT_KEY_PEER_ID)
                .map(str::to_string)
            else {
                // No peer-id TXT record means this isn't a Qcast session we
                // can join — skip silently.
                tracing::debug!(host = %info.get_hostname(), "mDNS resolved entry missing peer-id; skipping");
                return;
            };

            let Some(addr) = info.get_addresses().iter().next() else {
                tracing::debug!(host = %info.get_hostname(), "mDNS resolved entry has no addresses; skipping");
                return;
            };

            let session = LanSession {
                peer_id: peer_id.clone(),
                display_name: extract_instance_name(info.get_fullname()),
                addr: format!("{}:{}", addr, info.get_port()),
                last_seen: Instant::now(),
            };

            if let Ok(mut guard) = sessions.lock() {
                guard.insert(peer_id, session);
            }
        }
        ServiceEvent::ServiceRemoved(_ty, fullname) => {
            let display_name = extract_instance_name(fullname);
            // The remove event only tells us the fullname; drop any entry
            // whose display_name matches. Multiple producers sharing one
            // hostname is not a v1 concern (one publisher per host).
            if let Ok(mut guard) = sessions.lock() {
                guard.retain(|_, s| s.display_name != display_name);
            }
        }
        _ => {}
    }
}

/// `mdns-sd` rejects hostnames containing dots/spaces because it appends them
/// straight into a DNS name. Normalize aggressively so a Windows machine name
/// with a trailing space (or an FQDN already containing dots) still works.
fn sanitize_hostname(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    if cleaned.is_empty() {
        "qcast-host".to_string()
    } else {
        cleaned
    }
}

/// `info.get_fullname()` returns e.g. `ERIS-DESKTOP._qcast._tcp.local.` — pull
/// just the instance label off the front so the UI displays a friendly name.
fn extract_instance_name(fullname: &str) -> String {
    fullname
        .split_once('.')
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| fullname.to_string())
}

/// Best-effort local hostname for the mDNS instance name. Falls back to a
/// stable placeholder so publish never fails on a machine that can't report
/// its hostname.
pub fn local_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "qcast-host".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extract_instance_name` peels the instance label off a Bonjour-style
    /// fullname, leaving anything before the first dot. A name with no dots
    /// returns unchanged so the UI always has something to show.
    #[test]
    fn extract_instance_name_pulls_first_label() {
        assert_eq!(
            extract_instance_name("ERIS-DESKTOP._qcast._tcp.local."),
            "ERIS-DESKTOP"
        );
        assert_eq!(extract_instance_name("plain"), "plain");
        // Multiple dots: still take only the first label.
        assert_eq!(extract_instance_name("a.b.c"), "a");
    }

    /// Hostname sanitization replaces non-alphanumeric/dash chars and falls
    /// back to a placeholder for the empty case, so publish can't fail on a
    /// weird OS-reported hostname.
    #[test]
    fn sanitize_hostname_normalizes_funny_inputs() {
        assert_eq!(sanitize_hostname("ERIS-DESKTOP"), "ERIS-DESKTOP");
        assert_eq!(sanitize_hostname("  Alice's MacBook  "), "Alice-s-MacBook");
        assert_eq!(sanitize_hostname("host.with.dots"), "host-with-dots");
        assert_eq!(sanitize_hostname(""), "qcast-host");
        assert_eq!(sanitize_hostname("   "), "qcast-host");
    }

    /// Round-trip a known set of TXT records through `ServiceInfo` and prove
    /// our well-known keys come back intact. This guards against a future
    /// `mdns-sd` upgrade silently changing TXT decoding (e.g. case folding).
    #[test]
    fn txt_records_round_trip_through_service_info() {
        let properties: HashMap<String, String> = [
            (TXT_KEY_VERSION.to_string(), SCHEMA_VERSION.to_string()),
            (TXT_KEY_PEER_ID.to_string(), "GHF/ABA/6TJ".to_string()),
            (TXT_KEY_APP.to_string(), TXT_APP_VALUE.to_string()),
            (TXT_KEY_BUILD.to_string(), "0.2.0".to_string()),
        ]
        .into_iter()
        .collect();

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "TEST-HOST",
            "TEST-HOST.local.",
            "192.168.1.10",
            8443,
            properties,
        )
        .expect("ServiceInfo::new should accept our known-good TXT set");

        let props = info.get_properties();
        assert_eq!(
            props.get_property_val_str(TXT_KEY_VERSION),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(
            props.get_property_val_str(TXT_KEY_PEER_ID),
            Some("GHF/ABA/6TJ")
        );
        assert_eq!(
            props.get_property_val_str(TXT_KEY_APP),
            Some(TXT_APP_VALUE)
        );
        assert_eq!(props.get_property_val_str(TXT_KEY_BUILD), Some("0.2.0"));
    }

    /// Local hostname must always return a non-empty string — even on a host
    /// that refuses to report itself (then `local_hostname` falls back to the
    /// placeholder rather than empty-string).
    #[test]
    fn local_hostname_is_non_empty() {
        let h = local_hostname();
        assert!(!h.is_empty(), "local_hostname must never return empty");
    }
}
