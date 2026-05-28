// Minimal ambient typings for the bundled gstwebrtc-api library
// (`/gstwebrtc-api-3.0.0.min.js`). The upstream package ships no `.d.ts`, so we
// describe only the surface the Viewer actually touches. Keep this narrow —
// the broader API is intentionally consumed via `unknown`-ish escape hatches.

export interface GstWebRTCProducer {
  id: string;
  meta?: Record<string, unknown>;
}

export interface GstWebRTCConsumerSession extends EventTarget {
  streams: ReadonlyArray<MediaStream>;
  /** Attaches a video element so the data channel forwards mouse/keyboard. */
  attachVideoElement?: (video: HTMLVideoElement) => void;
  connect: () => void;
}

export interface GstWebRTCApiOptions {
  signalingServerUrl: string;
  webrtcConfig?: RTCConfiguration;
}

export interface GstWebRTCApiPeerListener {
  producerAdded?: (producer: GstWebRTCProducer) => void;
  producerRemoved?: (producer: GstWebRTCProducer) => void;
}

export declare class GstWebRTCAPI {
  constructor(options: GstWebRTCApiOptions);
  registerPeerListener(listener: GstWebRTCApiPeerListener): void;
  getAvailableProducers(): GstWebRTCProducer[];
  createConsumerSession(producerId: string): GstWebRTCConsumerSession;
}

declare global {
  interface Window {
    GstWebRTCAPI?: typeof GstWebRTCAPI;
  }
}
