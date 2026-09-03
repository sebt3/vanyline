<script setup lang="ts">
import type { Component } from 'vue';
import { provide } from 'vue';
import {
  ConfigShell,
  LlmProvidersScreen,
  ModelProfilesScreen,
  McpServersScreen,
  ToolsetsScreen,
  AgentsScreen,
  SkillsScreen,
  CONFIG_REPO_KEY,
  type ConfigNavGroup,
  type ConfigRepo,
} from '@vanyline/ui';

// Dépôt de config encore non branché (le relais host `config/*` + rpcConfigRepo
// arrivent en tâche 06). Écrit méthode par méthode, typé ConfigRepo — jamais de
// Proxy magique : chaque méthode rejette VNL-EXT-022, ce qui fait s'afficher
// l'ErrorCard de l'écran (le « hello ConfigShell » voulu par le design).
function stubConfigRepo(): ConfigRepo {
  const notWired = (): never => {
    throw new Error('VNL-EXT-022: dépôt de config non branché (tâche 06)');
  };
  const repo: ConfigRepo = {
    list: async (_domain) => notWired(),
    get: async (_domain, _name) => notWired(),
    create: async (_domain, _item) => notWired(),
    update: async (_domain, _name, _patch) => notWired(),
    remove: async (_domain, _name) => notWired(),
    setDefaultProvider: async (_name) => notWired(),
    testProvider: async (_name) => notWired(),
    testMcpServer: async (_name) => notWired(),
    listLocalTools: async () => notWired(),
  };
  return repo;
}

provide(CONFIG_REPO_KEY, stubConfigRepo());

// Nav des 4 groupes CLI — les groupes du frontend `SettingsView.vue` SANS
// `account` (pas de notion de compte côté CLI / F4). Mêmes labels/icons/accents.
const groups: ConfigNavGroup[] = [
  {
    id: 'modeles',
    label: 'Modèles',
    icon: '✦',
    accent: '#5b1ecf',
    sub: [
      { id: 'llm-providers', label: 'Fournisseurs LLM' },
      { id: 'model-profiles', label: 'Profils de modèle' },
    ],
  },
  {
    id: 'outils',
    label: 'Outils',
    icon: '⚙',
    accent: '#4c90f0',
    sub: [
      { id: 'mcp-servers', label: 'Serveurs MCPs' },
      { id: 'toolsets', label: 'Toolsets' },
    ],
  },
  { id: 'agents', label: 'Agents', icon: '✦', accent: '#e0a83d' },
  { id: 'skills', label: 'Skills', icon: '⚡', accent: '#3fb56d' },
];

// Les 6 écrans de @vanyline/ui (mêmes clés que le frontend ; `account` exclu).
const screens: Record<string, Component> = {
  'llm-providers': LlmProvidersScreen,
  'model-profiles': ModelProfilesScreen,
  'mcp-servers': McpServersScreen,
  toolsets: ToolsetsScreen,
  agents: AgentsScreen,
  skills: SkillsScreen,
};
</script>

<template>
  <ConfigShell :groups="groups" :screens="screens" />
</template>
