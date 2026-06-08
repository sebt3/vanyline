<script lang="ts">
  import { userStore } from '$lib/stores/user.svelte';
</script>

<div class="flex min-h-screen items-center justify-center">
  <div class="text-center">
    <h1 class="text-3xl font-bold mb-6">vanyline</h1>
    {#if userStore.loading}
      <p class="text-stone-400">Loading...</p>
    {:else}
      <a
        href="/auth/login"
        class="rounded-md bg-primary-600 px-6 py-3 text-white hover:bg-primary-500"
      >
        Sign in
      </a>
    {/if}
  </div>
</div>
