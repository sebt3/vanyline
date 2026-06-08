<script lang="ts">
  import Router from 'svelte-spa-router';
  import { onMount } from 'svelte';
  import { userStore } from './lib/stores/user.svelte';
  import Login from './pages/Login.svelte';
  import Chat from './pages/Chat.svelte';

  const routes = {
    '/': Chat,
    '/login': Login,
    '/chat': Chat,
    '/chat/:id': Chat,
  };

  onMount(() => {
    userStore.load();
  });
</script>

<div class="min-h-screen bg-stone-950 text-stone-100">
  <Router {routes} />
</div>
