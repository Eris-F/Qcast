<script lang="ts">
  // Phase 2 landing page — proves the toolchain (Vite + Svelte 5 + Tailwind v4 +
  // shadcn-svelte) is wired up. Real screens land in Phase 3 (see UI_REWRITE.md).
  import { Button } from '$lib/components/ui/button';
</script>

<main class="min-h-screen flex flex-col items-center justify-center gap-6 p-8">
  <div class="flex items-center gap-3">
    <span class="size-3 rounded-full bg-[var(--primary)]"></span>
    <h1 class="text-2xl font-semibold tracking-tight">Qcast</h1>
  </div>
  <p class="text-sm" style="color: var(--muted-foreground)">
    UI scaffold ready. Phase 3 builds the screens.
  </p>
  <Button>It builds.</Button>
</main>
