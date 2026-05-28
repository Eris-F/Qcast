<script lang="ts">
  // Client — LAN list + typed-code fallback. The list auto-refreshes every
  // 1.5s (the locked decision in UI_REWRITE.md §2). Phase 4 will additionally
  // listen for the `lan_sessions_changed` Tauri event for push updates.
  import Layout from '$lib/components/Layout.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Separator } from '$lib/components/ui/separator';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import { onDestroy, onMount } from 'svelte';
  import { push } from 'svelte-spa-router';
  import { ipc, type LanSession } from '$lib/ipc';

  const LAN_REFRESH_INTERVAL_MS = 1500;

  let sessions = $state<LanSession[]>([]);
  let refreshing = $state(false);
  let code = $state('');
  let connecting = $state<string | null>(null); // peerId or "code"
  let pollHandle: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    refreshing = true;
    try {
      sessions = await ipc.listLanSessions();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error('list_lan_sessions failed', err);
    } finally {
      refreshing = false;
    }
  }

  onMount(() => {
    void refresh();
    pollHandle = setInterval(refresh, LAN_REFRESH_INTERVAL_MS);
  });

  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
  });

  async function joinLan(peerId: string) {
    connecting = peerId;
    try {
      await ipc.connectToLan(peerId);
      void push('/client/viewer');
    } catch (err) {
      connecting = null;
      // eslint-disable-next-line no-console
      console.error('connect_to_lan failed', err);
    }
  }

  async function connectByCode() {
    const trimmed = code.trim();
    if (!trimmed) return;
    connecting = 'code';
    try {
      await ipc.connectToCode(trimmed);
      void push('/client/viewer');
    } catch (err) {
      connecting = null;
      // eslint-disable-next-line no-console
      console.error('connect_to_code failed', err);
    }
  }
</script>

<Layout back="/">
  <div class="mx-auto flex max-w-2xl flex-col gap-8">
    <div class="space-y-1">
      <h1 class="text-xl font-medium">Client</h1>
      <p class="text-muted-foreground text-sm">Connect to and control a remote Host.</p>
    </div>

    <section class="space-y-3">
      <div class="flex items-center justify-between">
        <h2 class="text-muted-foreground text-xs font-semibold uppercase tracking-wide">
          Hosts on this network
        </h2>
        <span
          class="text-muted-foreground inline-flex items-center gap-1 text-xs"
          aria-live="polite"
        >
          <RefreshIcon class="size-3 {refreshing ? 'animate-spin' : ''}" />
          refreshing…
        </span>
      </div>

      <div class="border-border bg-muted/20 divide-border divide-y rounded-[var(--radius)] border">
        {#if sessions.length === 0}
          <div class="text-muted-foreground p-4 text-sm">
            No Hosts are sharing on this network right now.
          </div>
        {:else}
          {#each sessions as session (session.peerId)}
            <div class="flex items-center justify-between gap-3 px-3 py-2.5">
              <div class="flex items-center gap-2.5 min-w-0">
                <span
                  class="size-2 shrink-0 rounded-full bg-emerald-400"
                  aria-hidden="true"
                ></span>
                <span class="text-sm truncate">{session.displayName}</span>
                <span class="text-muted-foreground hidden text-xs tabular-nums sm:inline">
                  {session.addr}
                </span>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={connecting !== null}
                onclick={() => void joinLan(session.peerId)}
              >
                {connecting === session.peerId ? 'Joining…' : 'Join'}
              </Button>
            </div>
          {/each}
        {/if}
      </div>
    </section>

    <Separator />

    <section class="space-y-3">
      <h2 class="text-muted-foreground text-xs font-semibold uppercase tracking-wide">
        Or enter a code your friend gave you
      </h2>
      <div class="flex gap-2">
        <Label for="code" class="sr-only">Pairing code</Label>
        <Input
          id="code"
          bind:value={code}
          placeholder="GHF / ABA / 6TJ"
          class="font-mono tracking-[0.2em] uppercase"
          autocomplete="off"
          spellcheck={false}
          onkeydown={(e: KeyboardEvent) => {
            if (e.key === 'Enter') void connectByCode();
          }}
        />
        <Button onclick={connectByCode} disabled={!code.trim() || connecting !== null}>
          {connecting === 'code' ? 'Connecting…' : 'Connect'}
        </Button>
      </div>
    </section>
  </div>
</Layout>
