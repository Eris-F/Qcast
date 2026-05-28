<script lang="ts">
  // Host — in-session. The window has been summoned back from the tray.
  // Shows elapsed time, the code, hotkey reminder, connected viewers and the
  // Stop CTA. See deploy/UI_REWRITE.md §3.3.
  //
  // The connected-viewer list is placeholder: Phase 4 will wire it to the
  // host's data-channel registry via a Tauri event.
  import Layout from '$lib/components/Layout.svelte';
  import PairingCodeDisplay from '$lib/components/PairingCodeDisplay.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Separator } from '$lib/components/ui/separator';
  import StopIcon from '@lucide/svelte/icons/square';
  import { onDestroy, onMount } from 'svelte';
  import { push } from 'svelte-spa-router';
  import { ipc, type ShareSession } from '$lib/ipc';

  let session = $state<ShareSession | null>(null);
  let stopping = $state(false);
  let elapsed = $state('00:00:00');
  let tickHandle: ReturnType<typeof setInterval> | null = null;

  function format(deltaMs: number): string {
    const total = Math.max(0, Math.floor(deltaMs / 1000));
    const h = String(Math.floor(total / 3600)).padStart(2, '0');
    const m = String(Math.floor((total % 3600) / 60)).padStart(2, '0');
    const s = String(total % 60).padStart(2, '0');
    return `${h}:${m}:${s}`;
  }

  onMount(async () => {
    const cur = await ipc.currentShare();
    if (!cur) {
      // No share active — kick back to the launcher rather than render a
      // misleading "sharing" header.
      void push('/');
      return;
    }
    session = cur;
    const started = new Date(cur.startedAt).getTime();
    elapsed = format(Date.now() - started);
    tickHandle = setInterval(() => {
      if (session) elapsed = format(Date.now() - started);
    }, 1000);
  });

  onDestroy(() => {
    if (tickHandle) clearInterval(tickHandle);
  });

  async function stop() {
    stopping = true;
    try {
      await ipc.stopShare();
      void push('/');
    } catch (err) {
      stopping = false;
      // eslint-disable-next-line no-console
      console.error('stop_share failed', err);
    }
  }
</script>

<Layout>
  <div class="mx-auto flex max-w-2xl flex-col gap-6">
    <div class="flex items-center gap-3">
      <span
        class="inline-block size-2 animate-pulse rounded-full bg-[var(--destructive)]"
        aria-hidden="true"
      ></span>
      <h1 class="text-base font-medium">
        Sharing <span class="text-muted-foreground"> · </span>
        <span class="tabular-nums">{elapsed}</span>
      </h1>
    </div>

    <section class="space-y-3">
      {#if session}
        <PairingCodeDisplay code={session.code} size="md" />
      {/if}
      <div class="text-muted-foreground text-sm">
        Stop hotkey: <kbd class="border-border rounded border bg-muted/40 px-1.5 py-0.5 font-mono text-xs">Ctrl + Alt + Q</kbd>
      </div>
    </section>

    <Separator />

    <section class="space-y-2">
      <h2 class="text-muted-foreground text-xs font-semibold uppercase tracking-wide">
        Connected
      </h2>
      <div class="border-border bg-muted/20 rounded-[var(--radius)] border p-3 text-sm">
        <div class="flex items-center gap-2">
          <span class="size-2 rounded-full bg-emerald-400" aria-hidden="true"></span>
          <span>Anonymous viewer</span>
          <span class="text-muted-foreground"> · </span>
          <span class="text-muted-foreground tabular-nums">18 ms</span>
          <span class="text-muted-foreground"> · </span>
          <span class="text-muted-foreground tabular-nums">1.2 Mb/s</span>
        </div>
      </div>
      <p class="text-muted-foreground text-xs">
        Phase 4 will populate this list from the live data-channel registry.
      </p>
    </section>

    <div class="flex justify-end pt-2">
      <Button variant="destructive" onclick={stop} disabled={stopping}>
        <StopIcon />
        {stopping ? 'Stopping…' : 'Stop sharing'}
      </Button>
    </div>
  </div>
</Layout>
