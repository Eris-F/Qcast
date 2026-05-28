<script lang="ts">
  // Role picker — first thing the user sees every launch (no remember-me).
  // Two big cards, formal Host / Client labels + plain-language helper.
  // See deploy/UI_REWRITE.md §3.1.
  import Layout from '$lib/components/Layout.svelte';
  import * as Card from '$lib/components/ui/card';
  import { push } from 'svelte-spa-router';
</script>

<Layout>
  <div class="mx-auto flex max-w-3xl flex-col gap-8 pt-6">
    <h1 class="text-foreground text-lg font-medium">What would you like to do?</h1>

    <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
      <Card.Root
        role="button"
        tabindex={0}
        aria-label="Become a Host"
        class="hover:border-primary/60 cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--ring)]"
        onclick={() => push('/host')}
        onkeydown={(e: KeyboardEvent) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            void push('/host');
          }
        }}
      >
        <Card.Header>
          <Card.Title class="text-2xl">Host</Card.Title>
          <Card.Description>
            Share this machine's screen and allow remote control.
          </Card.Description>
        </Card.Header>
        <Card.Content class="text-muted-foreground text-sm">
          You'll get a pairing code to share with your Client.
        </Card.Content>
      </Card.Root>

      <Card.Root
        role="button"
        tabindex={0}
        aria-label="Become a Client"
        class="hover:border-primary/60 cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--ring)]"
        onclick={() => push('/client')}
        onkeydown={(e: KeyboardEvent) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            void push('/client');
          }
        }}
      >
        <Card.Header>
          <Card.Title class="text-2xl">Client</Card.Title>
          <Card.Description>Connect to and control a remote Host.</Card.Description>
        </Card.Header>
        <Card.Content class="text-muted-foreground text-sm">
          Pick a Host from your network or paste a code.
        </Card.Content>
      </Card.Root>
    </div>
  </div>
</Layout>
