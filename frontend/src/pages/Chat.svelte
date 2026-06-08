<script lang="ts">
  import { userStore } from '$lib/stores/user.svelte';
  import { push } from 'svelte-spa-router';
  import { onMount } from 'svelte';

  onMount(() => {
    if (!userStore.loading && !userStore.email) {
      push('/login');
    }
  });
</script>

{#if userStore.loading}
  <div class="flex min-h-screen items-center justify-center">
    <p class="text-stone-400">Loading...</p>
  </div>
{:else if userStore.email}
  <div class="flex min-h-screen">
    <p class="text-stone-400 p-4">Chat — coming soon</p>
  </div>
{/if}
