<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import MenuBar from './components/MenuBar.vue';
import StatusBar from './components/StatusBar.vue';

const route = useRoute();
const isIdeRoute = computed(() => route.name === 'ide');
// Plus de "media-station" en dur : le workspace vient de la route.
// Hors route IDE (pas de sandbox) → chaîne vide.
const workspace = computed(() =>
  typeof route.params.sandboxName === 'string' ? route.params.sandboxName : '',
);
</script>

<template>
  <div class="shell">
    <div v-if="isIdeRoute" class="topbar">
      <MenuBar />
    </div>
    <div class="dock">
      <router-view />
    </div>
    <StatusBar :workspace="workspace" :extended="isIdeRoute" />
  </div>
</template>

<style scoped>
.shell {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: #10192c;
}
.topbar {
  flex: none;
  height: 34px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px 0 6px;
  background: #000c18;
  border-bottom: 1px solid #1c1c2a;
  color: white;
  font-size: 13px;
}
.dock {
  flex: 1;
  min-height: 0;
}
</style>
