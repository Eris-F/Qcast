//! End-to-end integration tests for the host.
//!
//! These exercise the real machinery — building the `webrtcsink` pipeline,
//! binding the TURN relay, extracting the embedded web client, serving HTTP,
//! and (Phase 1) round-tripping the mDNS publish/browse — rather than the
//! pure decision logic the unit tests in `host::tests` cover.
//!
//! The heavy host-start tests bind real ports and need the `webrtcsink`
//! GStreamer plugin, so they are marked `#[ignore]` and run explicitly with
//! `cargo test -p qcast-sender -- --ignored`. They also skip gracefully (early
//! return with a note) when `webrtcsink` is absent, so they never fail on a
//! machine without the plugin. All host-start tests serialize on a shared lock
//! so they don't fight over the fixed TURN port or the bound web/signalling
//! ports, and each tears the host down (releasing its ports) before asserting.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;

use crate::host::{self, CodecPref, HostConfig};

/// Serializes the heavy host-start tests. They share process-wide resources (the
/// fixed TURN UDP port, the embedded web client, and GStreamer global state), so
/// running them concurrently would make them fight over those resources. Acquire
/// this at the very top of each such test and hold it for the whole test body.
fn host_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // If a previous test panicked while holding the lock it'd be poisoned; we
    // don't care about the guarded data (it's `()`), so recover and continue.
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// True when the streaming core is present. The host-start tests can't build a
/// pipeline without it, so they skip (rather than fail) when it's missing.
fn webrtcsink_available() -> bool {
    let _ = gst::init();
    gst::ElementFactory::find("webrtcsink").is_some()
}

/// Find a free TCP port by binding an ephemeral one and reading back the OS
/// assignment, then releasing it. The host binds its own listener moments later;
/// this races only with unrelated processes, which is acceptable for a test and
/// far safer than reusing the fixed 8080/8443 defaults (which may clash with a
/// running instance).
fn free_tcp_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral tcp port");
    listener.local_addr().expect("read bound addr").port()
}

/// True if `127.0.0.1:port` can't be bound for TCP — i.e. something still holds
/// it. Used to confirm the host released its web/signalling ports after `stop`.
fn tcp_port_in_use(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// Build a `HostConfig` for a test pattern host on freshly-probed high ports with
/// the given resolution cap and codec preference. Never touches the 8080/8443
/// defaults, so it can't clash with a running instance.
fn test_config(access_code: &str, max_width: u32, max_height: u32, codec: CodecPref) -> HostConfig {
    HostConfig {
        host: "127.0.0.1".to_string(),
        web_port: free_tcp_port(),
        signalling_port: free_tcp_port(),
        test_pattern: true,
        max_width,
        max_height,
        codec_pref: codec,
        access_code: access_code.to_string(),
    }
}

/// Minimal HTTP/1.0 GET against `127.0.0.1:port`, returning `(status_code, body)`.
/// Deliberately hand-rolled over a raw `TcpStream` so the test suite needs no
/// HTTP-client dependency. HTTP/1.0 with `Connection: close` means the server
/// closes the socket when the body is done, so reading to EOF gives the full
/// response without parsing `Content-Length`.
fn http_get(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();

    // Status line: "HTTP/1.x <code> <reason>".
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);

    // Body is everything after the first blank line (header/body separator).
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();

    Ok((status, body))
}

/// Retry `http_get` until it returns a non-zero status or the deadline passes.
/// The web server comes up moments after the pipeline reaches Playing, so the
/// first connection can briefly fail/refuse; this gives it a short grace window.
fn http_get_with_retry(port: u16, path: &str, timeout: Duration) -> Option<(u16, String)> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(resp @ (status, _)) = http_get(port, path) {
            if status != 0 {
                return Some(resp);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Full host serves end-to-end: starting the host proves the TURN relay came up,
/// the test source resolved, the `webrtcsink` pipeline built and reached Playing,
/// and the embedded web client extracted. The HTTP GET then proves the web
/// server is actually serving the (gateless) viewer markup, and the post-stop
/// check proves the ports were released.
///
/// Phase 1 dropped the `session.json` access-code gate — pairing now happens at
/// the signalling layer via `producer-peer-id` and the mDNS LAN-discovery TXT
/// records (see `mdns.rs` and the `mdns_publisher_visible_to_browser` test).
///
/// `#[ignore]` because it binds real ports and needs the `webrtcsink` GStreamer
/// plugin; run with `cargo test -p qcast-sender -- --ignored`. Skips gracefully
/// when the plugin is absent.
#[test]
#[ignore]
fn full_host_serves_viewer_and_session() {
    let _lock = host_test_lock();
    if !webrtcsink_available() {
        eprintln!("skipping full_host_serves_viewer_and_session: webrtcsink plugin not found");
        return;
    }

    let access_code = crate::access_code::generate();
    let cfg = test_config(&access_code, host::VIDEO_MAX_WIDTH, host::VIDEO_MAX_HEIGHT, CodecPref::Auto);
    let web_port = cfg.web_port;
    let signalling_port = cfg.signalling_port;

    // start() blocks until the pipeline reaches Playing (or errors), so an Ok here
    // already proves the whole startup chain succeeded.
    let mut running = host::start(cfg).expect("host should start with the test pattern");

    // Run the assertions inside a closure so a failure unwinds back here and we
    // still stop the host (releasing its ports) before re-raising the panic.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (status, body) = http_get_with_retry(web_port, "/", Duration::from_secs(10))
            .expect("web server should answer GET / before the deadline");
        assert_eq!(status, 200, "GET / should return 200");
        assert!(
            body.contains("<title>Qcast</title>"),
            "GET / body should contain the viewer markup; got first 200 chars: {:.200}",
            body
        );
        // The pre-pivot password-gate markup is gone.
        assert!(
            !body.contains(r#"id="gate""#),
            "GET / body should not contain the dropped password gate"
        );
    }));

    // Always tear down, regardless of whether the assertions passed.
    running.stop();

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }

    // After stop the host must release the web + signalling ports it bound.
    wait_until(Duration::from_secs(5), || {
        !tcp_port_in_use(web_port) && !tcp_port_in_use(signalling_port)
    });
    assert!(!tcp_port_in_use(web_port), "web port {web_port} should be released after stop");
    assert!(
        !tcp_port_in_use(signalling_port),
        "signalling port {signalling_port} should be released after stop"
    );
}

/// A non-default codec (H.264-only) and a 720p cap still build a valid pipeline
/// and serve, proving the configurable resolution/codec options reach the
/// pipeline and don't break startup.
///
/// `#[ignore]` (binds ports, needs `webrtcsink`); skips gracefully without it.
#[test]
#[ignore]
fn host_serves_with_h264_only_720p() {
    let _lock = host_test_lock();
    if !webrtcsink_available() {
        eprintln!("skipping host_serves_with_h264_only_720p: webrtcsink plugin not found");
        return;
    }

    let access_code = crate::access_code::generate();
    let cfg = test_config(&access_code, 1280, 720, CodecPref::H264Only);
    let web_port = cfg.web_port;
    let signalling_port = cfg.signalling_port;

    let mut running = host::start(cfg).expect("host should start with H264Only @ 720p");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (status, _body) = http_get_with_retry(web_port, "/", Duration::from_secs(10))
            .expect("web server should answer GET / before the deadline");
        assert_eq!(status, 200, "GET / should return 200 with H264Only @ 720p");
    }));

    running.stop();

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }

    wait_until(Duration::from_secs(5), || {
        !tcp_port_in_use(web_port) && !tcp_port_in_use(signalling_port)
    });
    assert!(!tcp_port_in_use(web_port), "web port {web_port} should be released after stop");
    assert!(
        !tcp_port_in_use(signalling_port),
        "signalling port {signalling_port} should be released after stop"
    );
}

/// TURN relay lifecycle: `ensure_running` brings up a relay, `port_in_use` then
/// reports the fixed TURN port as taken, and `shutdown` releases it.
///
/// `#[ignore]` because it binds the fixed TURN UDP port ([`turn::PORT`]), which
/// would clash with a live host instance. It also handles the reuse path: if the
/// port is already held (e.g. an external/other relay), `ensure_running` returns
/// the `External` reuse handle and we don't assert ownership.
#[test]
#[ignore]
fn turn_relay_lifecycle() {
    let _lock = host_test_lock();

    // The relay's async work runs on a tokio runtime, matching how the host
    // drives it in `prepare_resources`.
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");

    // If something already holds the TURN port, ensure_running takes the reuse
    // path and returns External; assert that and stop (shutdown is a no-op for it).
    if crate::turn::port_in_use(crate::turn::PORT) {
        let relay = crate::turn::ensure_running(&rt, "127.0.0.1")
            .expect("ensure_running should reuse an existing relay");
        assert!(
            matches!(relay, crate::turn::Relay::External),
            "a pre-held TURN port should be reused as External"
        );
        crate::turn::shutdown(&rt, relay);
        return;
    }

    let relay = crate::turn::ensure_running(&rt, "127.0.0.1")
        .expect("ensure_running should start the built-in relay");

    // Run the assertions inside a closure so a failure unwinds back here and we
    // still shut the relay down (releasing the UDP port) before re-raising the
    // panic — mirroring the teardown the host-start tests use.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(
            matches!(relay, crate::turn::Relay::Embedded(_)),
            "with a free port we should own an Embedded relay"
        );
        // While the embedded relay is up, the UDP port reads as taken.
        assert!(
            crate::turn::port_in_use(crate::turn::PORT),
            "TURN port {} should be in use while the relay is running",
            crate::turn::PORT
        );
    }));

    // Always tear down, regardless of whether the assertions passed.
    crate::turn::shutdown(&rt, relay);

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }

    // After shutdown the relay should release the UDP port.
    let released = wait_until(Duration::from_secs(5), || {
        UdpSocket::bind(("0.0.0.0", crate::turn::PORT)).is_ok()
    });
    assert!(
        released,
        "TURN port {} should be released after shutdown",
        crate::turn::PORT
    );
}

/// Poll `cond` until it returns true or `timeout` elapses. Returns whether the
/// condition was observed true.
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// End-to-end mDNS round-trip: a publisher advertises the session and a browser
/// running in the same process observes it, then sees it disappear after the
/// publisher is dropped. Proves both the publish path (TXT records carry the
/// peer-id) and the browse path (`MdnsBrowser::snapshot` reflects live state).
///
/// `#[ignore]` because it touches the real network (mDNS uses multicast UDP
/// 5353) — in some CI sandboxes / Docker networks the loopback interface
/// doesn't support multicast, in which case the test would never see the
/// service. Matches the same `--ignored` convention the webrtcsink tests
/// above use for "needs real system state".
#[test]
#[ignore]
fn mdns_publisher_visible_to_browser() {
    use crate::mdns::{MdnsBrowser, MdnsPublisher};

    let peer_id = "TEST/CODE/123";
    let publisher = MdnsPublisher::publish(peer_id, "qcast-mdns-test", 18443)
        .expect("publisher should register the test service");
    let browser = MdnsBrowser::start().expect("browser should start");

    // Multicast announce + browser resolve takes a couple of seconds even on
    // loopback; give it 5s to converge.
    let appeared = wait_until(Duration::from_secs(5), || {
        browser
            .snapshot()
            .iter()
            .any(|s| s.peer_id == peer_id)
    });
    assert!(
        appeared,
        "browser should observe the published peer-id {peer_id} within 5s; \
         snapshot = {:?}",
        browser.snapshot()
    );

    // Dropping the publisher sends the goodbye packet and shuts the daemon
    // down. The browser may take a moment longer to reflect the removal.
    drop(publisher);

    let disappeared = wait_until(Duration::from_secs(5), || {
        !browser.snapshot().iter().any(|s| s.peer_id == peer_id)
    });
    assert!(
        disappeared,
        "browser should observe removal within 5s after publisher drop; \
         snapshot = {:?}",
        browser.snapshot()
    );
}
