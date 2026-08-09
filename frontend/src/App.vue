<script setup lang="ts">
import { ref } from 'vue';
import { DockviewVue, type DockviewReadyEvent, type VueComponent } from 'dockview-vue';
import Explorer from './components/panels/Explorer.vue';
import Editor from './components/panels/Editor.vue';
import Workflow from './components/panels/Workflow.vue';
import Chat from './components/panels/Chat.vue';
import Terminal from './components/panels/Terminal.vue';
import StatusBar from './components/StatusBar.vue';
import MenuBar from './components/MenuBar.vue';
import SettingsView from './components/SettingsView.vue';

const activeView = ref<'shell' | 'settings'>('shell');
function toggleSettings() {
  activeView.value = activeView.value === 'settings' ? 'shell' : 'settings';
}

// Pas d'écran de connexion dédié : OIDC est le seul mécanisme d'auth,
// un utilisateur non authentifié est redirigé vers le provider en amont
// de ce shell — il n'y a rien à afficher ici dans ce cas.

const components = {
  explorer: Explorer,
  editor: Editor,
  workflow: Workflow,
  chat: Chat,
  terminal: Terminal,
} as unknown as Record<string, VueComponent>;

function onReady(event: DockviewReadyEvent) {
  const { api } = event;

  api.addPanel({
    id: 'explorer',
    component: 'explorer',
    title: 'Explorer',
    initialWidth: 230,
  });

  api.addPanel({
    id: 'editor',
    component: 'editor',
    title: 'sync_library.py',
    position: { referencePanel: 'explorer', direction: 'right' },
  });

  api.addPanel({
    id: 'workflow',
    component: 'workflow',
    title: 'sync-media.dag',
    position: { referencePanel: 'editor', direction: 'within' },
  });

  api.addPanel({
    id: 'terminal',
    component: 'terminal',
    title: 'Terminal',
    position: { referencePanel: 'editor', direction: 'below' },
    initialHeight: 170,
  });

  api.addPanel({
    id: 'chat',
    component: 'chat',
    title: 'Assistant',
    position: { referencePanel: 'editor', direction: 'right' },
    initialWidth: 330,
  });

  api.getPanel('editor')?.api.setActive();
}
</script>

<template>
  <div class="shell">
    <div class="topbar">
      <MenuBar @toggle-settings="toggleSettings" />
      <span class="grow" />
      <span class="workspace">media-station</span>
    </div>
    <div class="dock" v-show="activeView === 'shell'">
      <DockviewVue
        class="dockview-theme-abyss"
        style="width: 100%; height: 100%"
        :components="components"
        @ready="onReady"
      />
    </div>
    <div class="dock" v-show="activeView === 'settings'">
      <SettingsView />
    </div>
    <StatusBar workspace="media-station" />
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
.grow { flex: 1; }
.workspace {
  color: rgba(255, 255, 255, 0.5);
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  font-size: 12px;
}
.dock {
  flex: 1;
  min-height: 0;
}
</style>
