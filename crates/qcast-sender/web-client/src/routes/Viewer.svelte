<script lang="ts">
  // Viewer — the only "real" screen this phase. Faithfully ports the legacy
  // `_legacy/app.js` consumer logic (status pill, tap-to-play, fullscreen,
  // auto-reconnect, attachVideoElement for remote control) onto Svelte 5.
  //
  // The gstwebrtc-api library is loaded from `/gstwebrtc-api-3.0.0.min.js`
  // (the file in `public/`). It exposes `window.GstWebRTCAPI` once the
  // <script> in `index.html` finishes; we wait for it in onMount before
  // touching the global.
  import StatusPill, { type StatusState } from '$lib/components/StatusPill.svelte';
  import { Button } from '$lib/components/ui/button';
  import FullscreenIcon from '@lucide/svelte/icons/maximize';
  import { onDestroy, onMount } from 'svelte';
  import { push } from 'svelte-spa-router';
  import type {
    GstWebRTCApiPeerListener,
    GstWebRTCConsumerSession,
    GstWebRTCProducer,
  } from '$lib/gstwebrtc-api';

  let video: HTMLVideoElement;
  let viewerEl: HTMLDivElement;
  let status = $state<StatusState>('connecting');
  let tapToPlayVisible = $state(false);

  // Mutable refs that should NOT be reactive — they're plumbing.
  let api: InstanceType<NonNullable<typeof window.GstWebRTCAPI>> | null = null;
  let session: GstWebRTCConsumerSession | null = null;
  let scriptLoadHandle: number | null = null;

  function tryPlay() {
    if (!video) return;
    video
      .play()
      .then(() => {
        tapToPlayVisible = false;
      })
      .catch(() => {
        // Autoplay blocked — user gesture required.
        tapToPlayVisible = true;
      });
  }

  function onTapToPlay() {
    tapToPlayVisible = false;
    video?.play().catch(() => {});
  }

  function toggleFullscreen() {
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
    } else {
      (viewerEl.requestFullscreen?.() ?? Promise.reject(new Error('no fullscreen'))).catch(
        () => {},
      );
    }
  }

  function consume(producer: GstWebRTCProducer) {
    if (!api || session) return;
    status = 'connecting';

    if (producer.meta) {
      // eslint-disable-next-line no-console
      console.info('producer meta:', producer.meta);
    }

    const next = api.createConsumerSession(producer.id);
    session = next;

    // Attaches mouse/keyboard/scroll forwarding via the data channel — this is
    // the receiver half of the remote-support pivot. Guarded so an older
    // bundled API can't break the viewer.
    if (typeof next.attachVideoElement === 'function') {
      next.attachVideoElement(video);
    }

    next.addEventListener('streamsChanged', () => {
      const streams = next.streams;
      if (streams && streams.length > 0) {
        video.srcObject = streams[0];
        status = 'live';
        tryPlay();
      }
    });
    next.addEventListener('error', (e) => {
      status = 'disconnected';
      // eslint-disable-next-line no-console
      console.error(e);
    });
    next.addEventListener('closed', () => {
      status = 'disconnected';
      session = null;
      // Auto-reconnect: re-consume if any producer is still around.
      if (!api) return;
      const producers = api.getAvailableProducers();
      if (producers.length > 0) {
        setTimeout(() => consume(producers[0]), 1000);
      }
    });

    next.connect();
  }

  function startStreaming() {
    if (api) return;
    const Ctor = window.GstWebRTCAPI;
    if (!Ctor) {
      status = 'disconnected';
      return;
    }
    status = 'connecting';

    // Force ICE through the host's in-process TURN relay. With both ends
    // relay-only there's exactly one candidate pair, which is the most
    // reliable transport and dodges a libnice nomination assertion on the
    // host side (see crates/qcast-sender/src/host.rs).
    api = new Ctor({
      signalingServerUrl: `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.hostname}:8443`,
      webrtcConfig: {
        iceServers: [
          {
            urls: [
              `turn:${location.hostname}:3478?transport=udp`,
              `turn:${location.hostname}:3478?transport=tcp`,
            ],
            username: 'qcast',
            credential: 'qcastpass',
          },
        ],
        iceTransportPolicy: 'relay',
      },
    });

    const listener: GstWebRTCApiPeerListener = {
      producerAdded: (producer) => consume(producer),
      producerRemoved: () => {
        if (!session) status = 'waiting';
      },
    };

    api.registerPeerListener(listener);

    const existing = api.getAvailableProducers();
    if (existing.length > 0) {
      existing.forEach((p) => listener.producerAdded?.(p));
    } else {
      status = 'waiting';
    }
  }

  function waitForGstApi(attempt = 0) {
    if (window.GstWebRTCAPI) {
      startStreaming();
      return;
    }
    if (attempt > 50) {
      // 5s ceiling — the script either loads or it doesn't.
      status = 'disconnected';
      return;
    }
    scriptLoadHandle = window.setTimeout(() => waitForGstApi(attempt + 1), 100);
  }

  onMount(() => {
    waitForGstApi();
  });

  onDestroy(() => {
    if (scriptLoadHandle !== null) clearTimeout(scriptLoadHandle);
    session = null;
    api = null;
  });
</script>

<svelte:head>
  <!-- Bundled gst-plugins-rs WebRTC consumer library. Loaded from /public so
       Vite copies it verbatim into dist/ for the embedded include_dir!. -->
  <script src="/gstwebrtc-api-3.0.0.min.js"></script>
</svelte:head>

<div
  bind:this={viewerEl}
  class="fixed inset-0 bg-black"
  data-viewer
>
  <!-- Top bar — pointer-events:none on the gradient so video clicks work, but
       the status pill + buttons re-enable themselves. -->
  <div
    class="pointer-events-none absolute inset-x-0 top-0 z-10 flex h-12 items-center gap-3 bg-gradient-to-b from-black/80 to-transparent px-4"
  >
    <div class="flex items-center gap-3">
      <span class="text-sm font-semibold tracking-wide">Qcast</span>
      <StatusPill state={status} />
    </div>
    <div class="ml-auto pointer-events-auto flex items-center gap-2">
      <Button
        variant="outline"
        size="icon-sm"
        aria-label="Toggle fullscreen"
        onclick={toggleFullscreen}
      >
        <FullscreenIcon />
      </Button>
      <Button variant="ghost" size="sm" onclick={() => push('/')}>Back</Button>
    </div>
  </div>

  <div class="absolute inset-0 flex items-center justify-center">
    <!-- tabindex makes the video focusable so keydown events flow into the
         gstwebrtc-api remote-control data channel. -->
    <!-- svelte-ignore a11y_media_has_caption -->
    <video
      bind:this={video}
      class="max-h-full max-w-full cursor-pointer bg-black"
      tabindex={0}
      autoplay
      playsinline
      muted
      onclick={toggleFullscreen}
    ></video>
  </div>

  {#if tapToPlayVisible}
    <button
      type="button"
      class="absolute inset-0 z-20 flex cursor-pointer items-center justify-center bg-black/45 text-lg"
      onclick={onTapToPlay}
    >
      <span class="border-border bg-card rounded-[var(--radius)] border px-5 py-3">
        ▶ Tap to view
      </span>
    </button>
  {/if}
</div>
