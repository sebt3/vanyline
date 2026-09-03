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
} from '@vanyline/ui';
import { getBridgeClient } from './bridge';
import { createRpcConfigRepo } from './rpcConfigRepo';

// getBridgeClient() appelé ICI (setup), jamais à l'import du module :
// acquireVsCodeApi() n'existe que dans la webview et une seule fois (cf.
// bridge.ts, pattern App.vue). Le repo traduit le port @vanyline/ui vers le
// RPC du CLI via le pont ; le relais host `config/*` est la tâche 06b.
provide(CONFIG_REPO_KEY, createRpcConfigRepo(getBridgeClient()));

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
