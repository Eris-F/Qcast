<script lang="ts">
  // App chrome: brand mark on the left, settings cog on the right. Used by
  // every screen except the Viewer, which has its own minimal overlay header.
  import type { Snippet } from 'svelte';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import { Button } from '$lib/components/ui/button';
  import SettingsDialog from './SettingsDialog.svelte';
  import { APP_VERSION } from '$lib/ipc';

  interface Props {
    /** Optional back-arrow target. When omitted the brand mark renders instead. */
    back?: string;
    /** Slot for the screen's primary content. */
    children: Snippet;
  }

  let { back, children }: Props = $props();

  let settingsOpen = $state(false);
  const backHref = $derived(back ? `#${back}` : undefined);
</script>

<div class="flex min-h-screen flex-col">
  <header
    class="flex h-12 shrink-0 items-center justify-between border-b border-border px-4"
  >
    <div class="flex items-center gap-3">
      {#if backHref}
        <!-- svelte-spa-router is hash-based, so back links target `#${path}`. -->
        <a
          href={backHref}
          class="text-muted-foreground hover:text-foreground text-sm transition-colors"
          aria-label="Back"
        >
          ←
        </a>
      {/if}
      <span
        class="inline-block size-2 rounded-full bg-[var(--primary)]"
        aria-hidden="true"
      ></span>
      <span class="text-sm font-semibold tracking-wide">Qcast</span>
    </div>
    <div class="flex items-center gap-2">
      <span class="text-muted-foreground text-xs tabular-nums">v{APP_VERSION}</span>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Settings"
        onclick={() => (settingsOpen = true)}
      >
        <SettingsIcon />
      </Button>
    </div>
  </header>

  <main class="flex-1 px-6 py-8">
    {@render children()}
  </main>
</div>

<SettingsDialog bind:open={settingsOpen} />
