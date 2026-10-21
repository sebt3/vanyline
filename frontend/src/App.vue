<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import AppBreadcrumb from './components/AppBreadcrumb.vue';
import AppNav from './components/shell/AppNav.vue';
import MenuBar from './components/MenuBar.vue';
import StatusBar from './components/StatusBar.vue';

const route = useRoute();
const isIdeRoute = computed(() => route.name === 'ide');
// Plus de "media-station" en dur : le workspace vient de la route.
// Hors route IDE (pas de sandbox) → chaîne vide.
/** La vue de sandbox porte sa propre barre d'onglets mobile : celle du shell est retirée sous md. */
const ownsBottomBar = computed(() => route.name === 'sandbox-overview');
const workspace = computed(() =>
  typeof route.params.sandboxName === 'string' ? route.params.sandboxName : '',
);
</script>

<template>
  <UApp>
    <!-- Route IDE : coquille inchangée (barre de menu, dock, barre d'état). -->
    <div v-if="isIdeRoute" class="shell shell-ide">
      <div class="topbar">
        <MenuBar />
      </div>
      <div class="dock">
        <router-view />
      </div>
      <StatusBar :workspace="workspace" :extended="true" />
    </div>
    <!-- Hors IDE : shell commun (cf. components/shell/shell.sdd). La page ne défile jamais ;
         seule la région de contenu défile. -->
    <div v-else class="shell shell-app" :class="{ 'shell-app--own-bar': ownsBottomBar }">
      <AppNav />
      <div class="app-main">
        <div class="ctx-bar">
          <AppBreadcrumb />
        </div>
        <div class="app-content">
          <router-view />
        </div>
      </div>
    </div>
  </UApp>
</template>

<style scoped>
.shell {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.shell-ide {
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
  overflow: hidden;
}

/* Shell hors IDE : rail à gauche à partir de md, barre d'onglets en bas en dessous. */
.shell-app {
  flex-direction: row;
  background: var(--bg);
  color: var(--text);
}
.app-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.ctx-bar {
  flex: none;
  display: flex;
  align-items: center;
  min-height: 28px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
  color: var(--muted);
  font-size: 12px;
}
.app-content {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: auto;
}

@media (max-width: 767px) {
  .shell-app--own-bar :deep(.app-nav) {
    display: none;
  }
  .shell-app {
    /* La nav est en premier dans le DOM : column-reverse la place en bas. */
    flex-direction: column-reverse;
  }
}
</style>
