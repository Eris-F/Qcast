<script lang="ts" module>
  /** Connection state for the Viewer's status pill. Mirrors legacy `app.js` STATUS. */
  export type StatusState = 'connecting' | 'live' | 'waiting' | 'disconnected';

  const LABELS: Record<StatusState, string> = {
    connecting: 'Connecting…',
    live: 'Live',
    waiting: 'Waiting for host',
    disconnected: 'Disconnected',
  };

  // Sharp-dark colour mapping. Live = OK green; waiting = primary blue;
  // connecting = warm amber; disconnected = destructive red.
  const COLOR: Record<StatusState, string> = {
    connecting: 'bg-amber-400 shadow-[0_0_8px_oklch(0.78_0.13_85)]',
    live: 'bg-emerald-400 shadow-[0_0_8px_oklch(0.75_0.18_152)]',
    waiting: 'bg-[var(--primary)] shadow-[0_0_8px_var(--primary)]',
    disconnected: 'bg-[var(--destructive)] shadow-[0_0_8px_var(--destructive)]',
  };
</script>

<script lang="ts">
  interface Props {
    state: StatusState;
  }

  let { state }: Props = $props();
</script>

<span
  class="border-border inline-flex items-center gap-2 rounded-[var(--radius)] border bg-background/60 px-2.5 py-1 text-xs backdrop-blur"
  data-state={state}
>
  <span class="size-2 rounded-full transition-colors {COLOR[state]}" aria-hidden="true"
  ></span>
  <span>{LABELS[state]}</span>
</span>
