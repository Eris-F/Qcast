<script lang="ts">
  // Top-level router host. We use svelte-spa-router (hash-based) because
  // file:// + WebView2 makes browser-history routing fragile, and a hash route
  // is also bullet-proof under `npm run dev` and `npm run preview`.
  import Router from 'svelte-spa-router';
  import Launcher from './routes/Launcher.svelte';
  import HostPreflight from './routes/HostPreflight.svelte';
  import HostActive from './routes/HostActive.svelte';
  import ClientConnect from './routes/ClientConnect.svelte';
  import Viewer from './routes/Viewer.svelte';

  // Order matters: more-specific routes first.
  const routes = {
    '/': Launcher,
    '/host': HostPreflight,
    '/host/active': HostActive,
    '/client': ClientConnect,
    '/client/viewer': Viewer,
    // Fallback for any unknown hash — bounce back to the launcher.
    '*': Launcher,
  };
</script>

<Router {routes} />
