// Qcast browser receiver: connect to the host's signaling WebSocket, answer the
// host's WebRTC offer, and play the incoming stream. No dependencies — uses the
// browser's built-in WebRTC. The host always offers; we answer.
//
// Reliability: if the connection drops (host restart, network blip) the client
// reconnects automatically with capped backoff.

const statusEl = document.getElementById("status");
const startEl = document.getElementById("start");
const video = document.getElementById("video");

const setStatus = (s) => { statusEl.textContent = s; };

let pc = null;
let ws = null;
let reconnectTimer = null;
let retry = 0;

function closePc() {
  if (pc) { try { pc.close(); } catch (_) {} pc = null; }
}

function scheduleReconnect() {
  closePc();
  if (reconnectTimer) return;
  const delay = Math.min(1000 * 2 ** retry, 8000); // exponential backoff, cap 8s
  retry++;
  setStatus(`reconnecting in ${Math.round(delay / 1000)}s…`);
  reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, delay);
}

function tryPlay() {
  video.play()
    .then(() => { startEl.style.display = "none"; })
    .catch(() => { startEl.style.display = "flex"; }); // autoplay blocked: ask for a tap
}

startEl.addEventListener("click", () => {
  startEl.style.display = "none";
  video.play().catch(() => {});
});

// Click the video to toggle fullscreen.
video.addEventListener("click", () => {
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => {});
  } else {
    (video.requestFullscreen?.() ?? Promise.reject()).catch(() => {});
  }
});

async function onMessage(ev) {
  let msg;
  try { msg = JSON.parse(ev.data); } catch (_) { return; }

  if (msg.type === "offer") {
    closePc();
    pc = new RTCPeerConnection();
    pc.ontrack = (e) => { video.srcObject = e.streams[0]; setStatus("connected"); tryPlay(); };
    pc.onicecandidate = (e) => {
      if (e.candidate && ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({
          type: "ice_candidate",
          candidate: e.candidate.candidate,
          sdp_m_line_index: e.candidate.sdpMLineIndex,
        }));
      }
    };
    pc.oniceconnectionstatechange = () => {
      if (!pc) return;
      setStatus("ice: " + pc.iceConnectionState);
      if (pc.iceConnectionState === "failed") scheduleReconnect();
    };
    await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));
    }
  } else if (msg.type === "ice_candidate") {
    if (!pc) return;
    try {
      await pc.addIceCandidate({ candidate: msg.candidate, sdpMLineIndex: msg.sdp_m_line_index });
    } catch (e) { console.warn("addIceCandidate failed", e); }
  } else if (msg.type === "bye") {
    setStatus("host closed");
    scheduleReconnect();
  }
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = () => { retry = 0; setStatus("signaling…"); };
  ws.onmessage = onMessage;
  ws.onerror = () => {}; // onclose will follow and drive reconnect
  ws.onclose = () => { scheduleReconnect(); };
}

connect();
