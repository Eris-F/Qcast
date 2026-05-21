// Qcast custom browser client. Uses the gstwebrtc-api library to consume the
// host's webrtcsink stream (which handles adaptive bitrate + codec negotiation
// + loss recovery for us), and adds our own UI + a hook for "extra data" from
// the host via the control data channel and the producer's meta.

const video = document.getElementById("video");
const statusEl = document.getElementById("status");
const startEl = document.getElementById("start");
const setStatus = (s) => { statusEl.textContent = s; };

// Signalling runs on the host (webrtcsink), default port 8443, same host as this page.
const api = new GstWebRTCAPI({
  signalingServerUrl: `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:8443`,
});

let session = null;

function tryPlay() {
  video.play()
    .then(() => { startEl.style.display = "none"; })
    .catch(() => { startEl.style.display = "flex"; }); // autoplay blocked: tap
}

startEl.addEventListener("click", () => { startEl.style.display = "none"; video.play().catch(() => {}); });
video.addEventListener("click", () => {
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else (video.requestFullscreen?.() ?? Promise.reject()).catch(() => {});
});

function consume(producer) {
  if (session) return;
  setStatus("connecting…");

  // "extra data" #1: static per-stream metadata the host attached via webrtcsink meta=...
  if (producer.meta) console.info("producer meta:", producer.meta);

  session = api.createConsumerSession(producer.id);

  session.addEventListener("streamsChanged", () => {
    const streams = session.streams;
    if (streams && streams.length > 0) {
      video.srcObject = streams[0];
      setStatus("connected");
      tryPlay();
    }
  });
  session.addEventListener("error", (e) => { setStatus("stream error"); console.error(e); });
  session.addEventListener("closed", () => {
    setStatus("disconnected");
    session = null;
    // try to reconnect to a producer if one is still around
    const producers = api.getAvailableProducers();
    if (producers.length > 0) setTimeout(() => consume(producers[0]), 1000);
  });

  session.connect();
}

const listener = {
  producerAdded: (producer) => { consume(producer); },
  producerRemoved: () => { setStatus("host went away"); },
};

api.registerPeerListener(listener);
// Pick up a producer that's already registered before this page loaded.
api.getAvailableProducers().forEach((p) => listener.producerAdded(p));
