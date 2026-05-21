// Qcast custom browser client. Uses the gstwebrtc-api library to consume the
// host's webrtcsink stream (which handles adaptive bitrate + codec negotiation
// + loss recovery for us), and adds our own UI + a hook for "extra data" from
// the host via the control data channel and the producer's meta.
//
// SECURITY NOTE: the password entry below is a CLIENT-SIDE UX GATE, not enforced
// authentication. The expected code is fetched from session.json and compared in
// the browser, so a determined LAN user can read session.json (or this JS) and
// learn the code, or skip the gate entirely by talking to the signalling server
// directly. Real enforcement belongs at the signalling layer (reject consumer
// sessions without a valid token) and is future work.

const video = document.getElementById("video");
const tapToPlay = document.getElementById("tap-to-play");
const fullscreenBtn = document.getElementById("fullscreen-btn");

const gateEl = document.getElementById("gate");
const viewerEl = document.getElementById("viewer");
const codeInput = document.getElementById("code-input");
const connectBtn = document.getElementById("connect");
const gateError = document.getElementById("gate-error");

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
// Password gate. Normalize so the viewer can omit/add slashes or spaces and
// type any case: keep only A–Z/0–9, uppercase. Compare normalized forms.
// ---------------------------------------------------------------------------
function normalizeCode(s) {
  return (s || "").toUpperCase().replace(/[^A-Z0-9]/g, "");
}

let expectedCode = null; // normalized expected value from session.json

async function loadExpectedCode() {
  try {
    const res = await fetch("session.json", { cache: "no-store" });
    if (!res.ok) throw new Error(`session.json HTTP ${res.status}`);
    const data = await res.json();
    expectedCode = normalizeCode(data.auth);
  } catch (e) {
    console.error("could not load session.json:", e);
    gateError.textContent = "Could not contact host. Reload the page.";
    connectBtn.disabled = true;
  }
}

function tryUnlock() {
  if (expectedCode === null) return; // not loaded yet
  const entered = normalizeCode(codeInput.value);
  if (entered.length === 0) {
    gateError.textContent = "Enter the password.";
    return;
  }
  if (entered === expectedCode) {
    gateError.textContent = "";
    gateEl.hidden = true;
    viewerEl.classList.add("active");
    startStreaming(); // only NOW do we touch WebRTC / signalling
  } else {
    gateError.textContent = "Incorrect password";
    codeInput.focus();
    codeInput.select();
  }
}

connectBtn.addEventListener("click", tryUnlock);
codeInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") tryUnlock();
});
codeInput.addEventListener("input", () => { gateError.textContent = ""; });

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
// WebRTC consumption. Nothing below runs until startStreaming() is invoked by
// the gate, so the producer subscription is genuinely blocked until the entered
// code matches.
// ---------------------------------------------------------------------------
let api = null;
let session = null;

function consume(producer) {
  if (session) return;
  setStatus("connecting");

  // "extra data" #1: static per-stream metadata the host attached via webrtcsink meta=...
  if (producer.meta) console.info("producer meta:", producer.meta);

  session = api.createConsumerSession(producer.id);

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

// Kick off loading the expected code immediately; the gate stays up until the
// viewer enters a matching code.
loadExpectedCode().then(() => codeInput.focus());
