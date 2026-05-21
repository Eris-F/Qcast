// Qcast browser receiver: connect to the host's signaling WebSocket, answer the
// host's WebRTC offer, and play the incoming stream. No dependencies — uses the
// browser's built-in WebRTC. The host always offers; we answer.

const statusEl = document.getElementById("status");
const startEl = document.getElementById("start");
const video = document.getElementById("video");

const setStatus = (s) => { statusEl.textContent = s; };

let pc = null;
let ws = null;

function tryPlay() {
  video.play()
    .then(() => { startEl.style.display = "none"; })
    .catch(() => { startEl.style.display = "flex"; }); // autoplay blocked: ask for a tap
}

startEl.addEventListener("click", () => {
  startEl.style.display = "none";
  video.play().catch(() => {});
});

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onopen = () => setStatus("signaling…");
  ws.onclose = () => setStatus("disconnected");
  ws.onerror = () => setStatus("signaling error");

  ws.onmessage = async (ev) => {
    const msg = JSON.parse(ev.data);

    if (msg.type === "offer") {
      pc = new RTCPeerConnection();

      pc.ontrack = (e) => {
        video.srcObject = e.streams[0];
        setStatus("connected");
        tryPlay();
      };
      pc.onicecandidate = (e) => {
        if (e.candidate) {
          ws.send(JSON.stringify({
            type: "ice_candidate",
            candidate: e.candidate.candidate,
            sdp_m_line_index: e.candidate.sdpMLineIndex,
          }));
        }
      };
      pc.oniceconnectionstatechange = () => setStatus("ice: " + pc.iceConnectionState);

      await pc.setRemoteDescription({ type: "offer", sdp: msg.sdp });
      const answer = await pc.createAnswer();
      await pc.setLocalDescription(answer);
      ws.send(JSON.stringify({ type: "answer", sdp: answer.sdp }));

    } else if (msg.type === "ice_candidate") {
      try {
        await pc.addIceCandidate({
          candidate: msg.candidate,
          sdpMLineIndex: msg.sdp_m_line_index,
        });
      } catch (e) {
        console.warn("addIceCandidate failed", e);
      }

    } else if (msg.type === "bye") {
      setStatus("host closed");
      if (pc) pc.close();
    }
  };
}

connect();
