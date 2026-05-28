// Qcast custom browser client. Uses the gstwebrtc-api library to consume the
// host's webrtcsink stream (which handles adaptive bitrate + codec negotiation
// + loss recovery for us), and adds our own UI + a hook for "extra data" from
// the host via the control data channel and the producer's meta.
//
// SECURITY NOTE: pairing is now enforced at the signalling layer via the
// access code (set as webrtcsink's `producer-peer-id` on the sender). The
// pre-pivot client-side `session.json` password gate has been removed; the
// viewer just subscribes to the first available producer once the host is up.
// Subscribing to a specific peer-id by code is Phase 3 frontend work.

const video = document.getElementById("video");
const tapToPlay = document.getElementById("tap-to-play");
const fullscreenBtn = document.getElementById("fullscreen-btn");

const viewerEl = document.getElementById("viewer");

const statusEl = document.getElementById("status");
const statusText = document.getElementById("status-text");

// ---------------------------------------------------------------------------
// Status indicator. Maps a state -> (css class, label) for the header pill.
// ---------------------------------------------------------------------------
const STATUS = {
  connecting: ["connecting", "Connecting…"],
  live: ["live", "Live"],
  waiting: ["waiting", "Waiting for host"],
  disconnected: ["disconnected", "Disconnected"],
};
function setStatus(state) {
  const [cls, label] = STATUS[state] || STATUS.connecting;
  statusEl.className = cls;
  statusText.textContent = label;
}

// ---------------------------------------------------------------------------
// Video playback helpers.
// ---------------------------------------------------------------------------
function tryPlay() {
  video.play()
    .then(() => { tapToPlay.style.display = "none"; })
    .catch(() => { tapToPlay.style.display = "flex"; }); // autoplay blocked: tap
}

tapToPlay.addEventListener("click", () => {
  tapToPlay.style.display = "none";
  video.play().catch(() => {});
});

fullscreenBtn.addEventListener("click", toggleFullscreen);
video.addEventListener("click", toggleFullscreen);
function toggleFullscreen() {
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => {});
  } else {
    (viewerEl.requestFullscreen?.() ?? Promise.reject()).catch(() => {});
  }
}

// ---------------------------------------------------------------------------
// WebRTC consumption.
// ---------------------------------------------------------------------------
let api = null;
let session = null;

function consume(producer) {
  if (session) return;
  setStatus("connecting");

  // "extra data" #1: static per-stream metadata the host attached via webrtcsink meta=...
  if (producer.meta) console.info("producer meta:", producer.meta);

  session = api.createConsumerSession(producer.id);

  // Remote control — the receiver half of the remote-support pivot. Attaching the
  // video element makes gstwebrtc-api forward this viewer's mouse / keyboard /
  // scroll to the host as GstNavigation events over the data channel; the host has
  // webrtcsink `enable-data-channel-navigation` on and replays them via SendInput
  // (see crates/qcast-sender/src/input). Guarded so an older bundled api can't break
  // the viewer. NOTE: needs browser/Windows validation (see deploy/TEST_PLAN.md).
  if (typeof session.attachVideoElement === "function") {
    session.attachVideoElement(video);
  }

  session.addEventListener("streamsChanged", () => {
    const streams = session.streams;
    if (streams && streams.length > 0) {
      video.srcObject = streams[0];
      setStatus("live");
      tryPlay();
    }
  });
  session.addEventListener("error", (e) => { setStatus("disconnected"); console.error(e); });
  session.addEventListener("closed", () => {
    setStatus("disconnected");
    session = null;
    // Auto-reconnect: try to re-consume a producer if one is still around.
    const producers = api.getAvailableProducers();
    if (producers.length > 0) setTimeout(() => consume(producers[0]), 1000);
  });

  session.connect();
}

function startStreaming() {
  if (api) return; // guard against double-start
  setStatus("connecting");

  // Signalling runs on the host (webrtcsink), default port 8443, same host as this page.
  api = new GstWebRTCAPI({
    signalingServerUrl: `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:8443`,
    // Force media through the host's in-process TURN relay. With both ends
    // relay-only, ICE has a single relay↔relay candidate pair — the most reliable
    // transport, and it avoids a libnice nomination assertion that aborts the host
    // when the full host/srflx/mDNS/TCP candidate matrix races. (See host.rs.)
    webrtcConfig: {
      iceServers: [{
        urls: [
          `turn:${location.hostname}:3478?transport=udp`,
          `turn:${location.hostname}:3478?transport=tcp`,
        ],
        username: "qcast",
        credential: "qcastpass",
      }],
      iceTransportPolicy: "relay",
    },
  });

  const listener = {
    producerAdded: (producer) => { consume(producer); },
    producerRemoved: () => { if (!session) setStatus("waiting"); },
  };

  api.registerPeerListener(listener);

  // Pick up a producer that's already registered before this page loaded.
  const existing = api.getAvailableProducers();
  if (existing.length > 0) existing.forEach((p) => listener.producerAdded(p));
  else setStatus("waiting");
}

// Kick the consumer off as soon as the page loads — no gate.
startStreaming();
