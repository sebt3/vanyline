<script setup lang="ts">
import { DockviewVue, type DockviewReadyEvent, type VueComponent } from 'dockview-vue';
import Explorer from './panels/Explorer.vue';
import Editor from './panels/Editor.vue';
import Workflow from './panels/Workflow.vue';
import Chat from './panels/Chat.vue';
import Terminal from './panels/Terminal.vue';

defineProps<{ sandboxName: string }>();

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
  <div class="dock">
    <DockviewVue
      class="dockview-theme-abyss"
      style="width: 100%; height: 100%"
      :components="components"
      @ready="onReady"
    />
  </div>
</template>

<style scoped>
.dock { height: 100%; min-height: 0; }
</style>