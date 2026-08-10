<script setup lang="ts">
import type { Component } from 'vue';
import { computed, ref } from 'vue';
import AccountScreen from './settings/AccountScreen.vue';
import ProjectsScreen from './settings/ProjectsScreen.vue';
import SandboxesScreen from './settings/SandboxesScreen.vue';
import LlmProvidersScreen from './settings/LlmProvidersScreen.vue';
import ModelProfilesScreen from './settings/ModelProfilesScreen.vue';
import ToolsetsScreen from './settings/ToolsetsScreen.vue';

interface NavSub {
  id: string;
  label: string;
}

interface NavGroup {
  id: string;
  label: string;
  icon: string;
  accent: string;
  sub?: NavSub[];
}

const groups: NavGroup[] = [
  { id: 'projects', label: 'Projets', icon: '⌥', accent: '#4c90f0' },
  { id: 'sandboxes', label: 'Sandboxes', icon: '▣', accent: '#3fb56d' },
  {
    id: 'agent',
    label: 'Agent & modèle',
    icon: '✦',
    accent: '#5b1ecf',
    sub: [
      { id: 'llm-providers', label: 'Fournisseurs LLM' },
      { id: 'model-profiles', label: 'Profils de modèle' },
      { id: 'toolsets', label: 'Toolsets' },
      { id: 'skills', label: 'Skills' },
      { id: 'agents', label: 'Agents' },
      { id: 'mcp-servers', label: 'Serveurs MCP' },
    ],
  },
  { id: 'account', label: 'Compte', icon: '●', accent: '#e0a83d' },
];

const activeGroup = ref(groups[0].id);
const activeScreen = ref(getScreenId(groups[0]));
const expandedAgent = ref(false);

function getScreenId(group: NavGroup): string {
  if (group.sub && group.sub.length > 0) {
    return group.sub[0].id;
  }
  return group.id;
}

function setActiveGroup(groupId: string) {
  const group = groups.find((g) => g.id === groupId);
  if (!group) return;
  activeGroup.value = groupId;
  if (group.sub) {
    expandedAgent.value = true;
    activeScreen.value = group.sub[0].id;
  } else {
    expandedAgent.value = false;
    activeScreen.value = groupId;
  }
}

function setSubScreen(subId: string) {
  activeScreen.value = subId;
}

// Liste des écrans qui n'ont pas encore été implémentés (rendent un placeholder)
const pendingScreenIds = [
  'skills',
  'agents',
  'mcp-servers',
];

const isPending = computed(() => pendingScreenIds.includes(activeScreen.value));

const screens: Record<string, Component> = {
  account: AccountScreen,
  projects: ProjectsScreen,
  sandboxes: SandboxesScreen,
  'llm-providers': LlmProvidersScreen,
  'model-profiles': ModelProfilesScreen,
  toolsets: ToolsetsScreen,
};

const Pending: Component = {
  template: '<div class="pending"><span class="pending-icon">🔜</span>À venir</div>',
  styles: [
    `
.pending {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 100%;
  color: #6a7185;
  font-size: 14px;
}
.pending-icon {
  font-size: 22px;
}
`,
  ],
};
</script>

<template>
  <div class="settings">
    <nav class="nav" aria-label="Configuration">
      <template v-for="group in groups" :key="group.id">
        <button
          class="nav-item"
          :class="{ active: group.id === activeGroup }"
          :data-group="group.id"
          :style="{ '--accent': group.accent }"
          @click="setActiveGroup(group.id)"
        >
          <span class="nav-icon">{{ group.icon }}</span>
          <span class="nav-label">{{ group.label }}</span>
          <template v-if="group.sub">
            <span
              class="nav-arrow"
              :class="{ expanded: group.id === 'agent' && expandedAgent }"
              @click.stop="expandedAgent = !expandedAgent"
            >▼</span>
          </template>
        </button>
        <template v-if="group.sub && expandedAgent">
          <button
            v-for="sub in group.sub"
            :key="sub.id"
            class="nav-sub-item"
            :class="{ active: activeScreen === sub.id }"
            :style="{ '--accent': group.accent }"
            @click="setSubScreen(sub.id)"
          >
            {{ sub.label }}
          </button>
        </template>
      </template>
    </nav>
    <main class="panels">
      <div class="screen-wrap">
        <component :is="isPending ? Pending : screens[activeScreen]" />
      </div>
    </main>
  </div>
</template>

<style scoped>
.settings {
  height: 100%;
  width: 100%;
  display: flex;
  background: #0c1420;
  color: #e6e9f0;
  font-size: 13px;
}
.nav {
  width: 240px;
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 24px 14px;
  border-right: 1px solid #1c1c2a;
}
.nav-item {
  appearance: none;
  border: none;
  border-left: 3px solid transparent;
  background: transparent;
  color: #9497a9;
  display: flex;
  align-items: center;
  gap: 10px;
  text-align: left;
  font: inherit;
  font-size: 13.5px;
  height: 38px;
  padding: 0 10px;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  width: 100%;
  font-family: inherit;
}
.nav-icon {
  width: 18px;
  text-align: center;
  font-size: 15px;
  color: var(--accent);
}
.nav-item:hover {
  background: #161d2c;
  color: white;
}
.nav-item.active {
  background: #161d2c;
  border-left-color: var(--accent);
  color: white;
  font-weight: 600;
}
.nav-arrow {
  margin-left: auto;
  font-size: 10px;
  color: #6a7185;
  transition: transform 0.15s;
}
.nav-arrow.expanded {
  transform: rotate(180deg);
}
.nav-sub-item {
  appearance: none;
  background: transparent;
  border: none;
  color: #6a7185;
  display: block;
  text-align: left;
  font: inherit;
  font-size: 12.5px;
  height: 32px;
  padding: 0 10px 0 42px;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  width: 100%;
  font-family: inherit;
}
.nav-sub-item:hover {
  background: #161d2c;
  color: white;
}
.nav-sub-item.active {
  color: white;
  font-weight: 600;
}

.panels {
  flex: 1;
  overflow-y: auto;
  padding: 48px 56px;
}
.screen-wrap {
  max-width: 760px;
}
</style>