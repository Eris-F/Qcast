<script lang="ts">
  // Host — pre-flight. Shows the pairing code (visually dominant), the
  // session-scoped kill-keybind and Allow-input toggle, and the Start CTA.
  // See deploy/UI_REWRITE.md §3.2.
  import Layout from '$lib/components/Layout.svelte';
  import PairingCodeDisplay from '$lib/components/PairingCodeDisplay.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Switch } from '$lib/components/ui/switch';
  import { Separator } from '$lib/components/ui/separator';
  import PlayIcon from '@lucide/svelte/icons/play';
  import { onMount } from 'svelte';
  import { push } from 'svelte-spa-router';
  import { ipc } from '$lib/ipc';

  // The pairing code is regenerated server-side on every share. We preview a
  // candidate up-front by calling `current_share` (which is null on first
  // load); the real code is the one returned from `start_share`.
  let previewCode = $state('—— / ——— / ———');
  let killHotkey = $state('Ctrl+Alt+Q');
  let allowInput = $state(true);
  let starting = $state(false);

  onMount(async () => {
    const settings = await ipc.getSettings();
    killHotkey = settings.defaultKillHotkey;
    const existing = await ipc.currentShare();
    if (existing) {
      // Already sharing — jump to in-session straight away.
      previewCode = existing.code;
      void push('/host/active');
    }
  });

  async function start() {
    starting = true;
    try {
      await ipc.startShare({ killHotkey, allowInput });
      void push('/host/active');
    } catch (err) {
      starting = false;
      // eslint-disable-next-line no-console
      console.error('start_share failed', err);
    }
  }
</script>

<Layout back="/">
  <div class="mx-auto flex max-w-2xl flex-col gap-8">
    <div class="space-y-1">
      <h1 class="text-xl font-medium">Host</h1>
      <p class="text-muted-foreground text-sm">
        Share this machine's screen and allow remote control.
      </p>
    </div>

    <section class="space-y-3">
      <Label class="text-muted-foreground text-xs uppercase tracking-wide">
        Your pairing code
      </Label>
      <PairingCodeDisplay code={previewCode} />
      <p class="text-muted-foreground text-sm">
        Read this to your friend (or they'll see you on their network automatically).
      </p>
    </section>

    <Separator />

    <section class="space-y-4">
      <h2 class="text-muted-foreground text-xs font-semibold uppercase tracking-wide">
        Settings for this session
      </h2>

      <div class="grid grid-cols-[140px_1fr] items-center gap-x-4 gap-y-3">
        <Label for="killhotkey" class="text-sm">Stop hotkey</Label>
        <Input
          id="killhotkey"
          bind:value={killHotkey}
          placeholder="Ctrl+Alt+Q"
          class="max-w-xs font-mono text-sm"
        />

        <Label for="allow-input" class="text-sm">Let them control</Label>
        <div class="flex items-center gap-3">
          <Switch id="allow-input" bind:checked={allowInput} />
          <span class="text-muted-foreground text-sm">
            {allowInput ? 'Mouse and keyboard' : 'View-only'}
          </span>
        </div>
      </div>
    </section>

    <div class="flex justify-end pt-2">
      <Button onclick={start} disabled={starting}>
        <PlayIcon />
        {starting ? 'Starting…' : 'Start sharing'}
      </Button>
    </div>
  </div>
</Layout>
