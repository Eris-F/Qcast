<script lang="ts">
  // Settings modal — see deploy/UI_REWRITE.md §3.6.
  //
  // Three sections: Updates, Sharing, About. Loads settings from IPC the first
  // time it opens; patches are pushed back through `update_settings`.
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { Switch } from '$lib/components/ui/switch';
  import { Separator } from '$lib/components/ui/separator';
  import { ipc, APP_VERSION, type Settings, type UpdateInfo } from '$lib/ipc';

  interface Props {
    open: boolean;
  }

  let { open = $bindable(false) }: Props = $props();

  let settings = $state<Settings | null>(null);
  let checking = $state(false);
  let updateResult = $state<UpdateInfo | null | 'none'>(null);

  // Lazy-load on first open so a fresh frontend session always starts from the
  // backend's source of truth.
  $effect(() => {
    if (open && settings === null) {
      void ipc.getSettings().then((s) => {
        settings = s;
      });
    }
  });

  async function setKillHotkey(value: string) {
    if (!settings) return;
    settings = { ...settings, defaultKillHotkey: value };
    await ipc.updateSettings({ defaultKillHotkey: value });
  }

  async function setAutoCheck(value: boolean) {
    if (!settings) return;
    settings = { ...settings, autoCheckUpdates: value };
    await ipc.updateSettings({ autoCheckUpdates: value });
  }

  async function checkForUpdates() {
    checking = true;
    updateResult = null;
    try {
      const info = await ipc.checkForUpdates();
      updateResult = info ?? 'none';
    } finally {
      checking = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>Settings</Dialog.Title>
    </Dialog.Header>

    <div class="space-y-6 py-2">
      <section class="space-y-3">
        <h3 class="text-sm font-semibold">Updates</h3>
        <div class="text-muted-foreground space-y-1 text-sm">
          <div>Current version: <span class="text-foreground tabular-nums">{APP_VERSION}</span></div>
          {#if updateResult === 'none'}
            <div>You're up to date.</div>
          {:else if updateResult}
            <div class="text-foreground">Update available: v{updateResult.version}</div>
          {/if}
        </div>
        <div class="flex items-center gap-3">
          <Button variant="outline" size="sm" disabled={checking} onclick={checkForUpdates}>
            {checking ? 'Checking…' : 'Check for updates'}
          </Button>
        </div>
        <div class="flex items-center gap-2">
          <Switch
            id="auto-check"
            checked={settings?.autoCheckUpdates ?? false}
            onCheckedChange={(v) => void setAutoCheck(v)}
          />
          <Label for="auto-check" class="text-sm font-normal">
            Check automatically on launch
          </Label>
        </div>
      </section>

      <Separator />

      <section class="space-y-3">
        <h3 class="text-sm font-semibold">Sharing</h3>
        <div class="space-y-1.5">
          <Label for="default-killhotkey" class="text-muted-foreground text-xs">
            Default stop hotkey
          </Label>
          <Input
            id="default-killhotkey"
            value={settings?.defaultKillHotkey ?? ''}
            placeholder="Ctrl+Alt+Q"
            class="font-mono text-sm"
            onchange={(e) => void setKillHotkey((e.currentTarget as HTMLInputElement).value)}
          />
        </div>
      </section>

      <Separator />

      <section class="space-y-2">
        <h3 class="text-sm font-semibold">About</h3>
        <p class="text-muted-foreground text-sm leading-relaxed">
          Qcast is a screen-share tool for helping friends with their PCs. Source:
          <a
            href="https://github.com/Eris-F/Qcast"
            target="_blank"
            rel="noopener noreferrer"
            class="text-primary hover:underline"
          >
            github.com/Eris-F/Qcast
          </a>
        </p>
      </section>
    </div>
  </Dialog.Content>
</Dialog.Root>
