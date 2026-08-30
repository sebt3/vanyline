<script setup lang="ts">
import type { Component } from 'vue';
import { provide } from 'vue';
import {
  ConfigShell,
  LlmProvidersScreen,
  SkillsScreen,
  CONFIG_REPO_KEY,
  type ConfigNavGroup,
} from '@vanyline/ui';
import AccountScreen from './settings/AccountScreen.vue';
import ModelProfilesScreen from './settings/ModelProfilesScreen.vue';
import ToolsetsScreen from './settings/ToolsetsScreen.vue';
import AgentsScreen from './settings/AgentsScreen.vue';
import McpServersScreen from './settings/McpServersScreen.vue';
import { httpConfigRepo } from '../api/httpConfigRepo';
import { activeNav } from './settings/navState';

provide(CONFIG_REPO_KEY, httpConfigRepo());

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
  { id: 'account', label: 'Compte', icon: '●', accent: '#e0a83d' },
];

// Écrans extraits dans @vanyline/ui (backend-agnostiques via ConfigRepo) +
// écrans encore locaux (extraction en cours, tâches 08-09).
const screens: Record<string, Component> = {
  'llm-providers': LlmProvidersScreen,
  'model-profiles': ModelProfilesScreen,
  'mcp-servers': McpServersScreen,
  toolsets: ToolsetsScreen,
  agents: AgentsScreen,
  skills: SkillsScreen,
  account: AccountScreen,
};

function onNavChange(payload: { groupLabel: string; screenLabel: string }) {
  activeNav.value = { groupLabel: payload.groupLabel, screenLabel: payload.screenLabel };
}
</script>

<template>
  <ConfigShell :groups="groups" :screens="screens" @nav-change="onNavChange" />
</template>
