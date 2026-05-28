<script lang="ts">
  // The pairing-code element is load-bearing on the Host screens. Big mono
  // number, primary-blue accent, one-click copy. See UI_REWRITE.md §3.2.
  import { Button } from '$lib/components/ui/button';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import CheckIcon from '@lucide/svelte/icons/check';

  interface Props {
    code: string;
    /** Visual size; `lg` is the Host pre-flight, `md` is the in-session header. */
    size?: 'md' | 'lg';
  }

  let { code, size = 'lg' }: Props = $props();

  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | null = null;

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
      if (copyTimer) clearTimeout(copyTimer);
      copyTimer = setTimeout(() => {
        copied = false;
      }, 1500);
    } catch {
      // Clipboard write can fail when the page isn't focused or in a
      // non-secure context; swallow silently — the user can re-type the code.
    }
  }
</script>

<div class="flex items-center gap-3">
  <div
    class="border-border bg-muted/40 inline-flex items-center justify-center rounded-[var(--radius)] border px-5 py-3 font-mono tracking-[0.2em] text-[var(--primary)] {size === 'lg' ? 'text-3xl' : 'text-xl'}"
  >
    {code}
  </div>
  <Button variant="outline" size="sm" onclick={copy} aria-label="Copy pairing code">
    {#if copied}
      <CheckIcon />
      Copied
    {:else}
      <CopyIcon />
      Copy
    {/if}
  </Button>
</div>
