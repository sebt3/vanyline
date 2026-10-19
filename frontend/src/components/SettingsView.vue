<script setup lang="ts">
import type { Component } from 'vue';
import { computed, defineComponent, h, onMounted, provide, ref } from 'vue';
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
import AccountScreen from './settings/AccountScreen.vue';
import ContextChat from './ContextChat.vue';
import { httpConfigRepo } from '../api/httpConfigRepo';
import { listModelProfiles } from '../api/contextChatBackend';
import { isAdmin } from '../api/appSettingsClient';
import GlobalSettingsScreen from './settings/GlobalSettingsScreen.vue';
import { activeNav } from './settings/navState';
import { MD_MIN_WIDTH, useViewportWidth } from './dashboards/viewport';

provide(CONFIG_REPO_KEY, httpConfigRepo());

const baseGroups: ConfigNavGroup[] = [
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

/** Groupe « Global » (modèle par défaut, app-settings) : administrateurs seulement. */
const globalGroup: ConfigNavGroup = { id: 'global', label: 'Global', icon: '◎', accent: '#5b1ecf' };

const adminUser = ref(false);

onMounted(async () => {
  adminUser.value = await isAdmin();
});

const groups = computed<ConfigNavGroup[]>(() =>
  adminUser.value ? [...baseGroups.slice(0, -1), globalGroup, baseGroups[baseGroups.length - 1]] : baseGroups,
);

/**
 * Fin de tour de l'assistant : chaque écran de configuration est remonté (clé qui change),
 * donc il recharge sa liste (contrat SettingsView.sdd, « Cohérence »).
 */
const reloadKey = ref(0);

function reloadable(screen: Component): Component {
  const name = (screen as { __name?: string }).__name ?? screen.name;
  return defineComponent({
    name,
    setup: () => () => h(screen, { key: reloadKey.value }),
  });
}

// Écrans de config extraits dans @vanyline/ui (backend-agnostiques via
// ConfigRepo) ; `account` reste local (pas de notion de compte côté CLI/F4).
const screens: Record<string, Component> = {
  'llm-providers': reloadable(LlmProvidersScreen),
  'model-profiles': reloadable(ModelProfilesScreen),
  'mcp-servers': reloadable(McpServersScreen),
  toolsets: reloadable(ToolsetsScreen),
  agents: reloadable(AgentsScreen),
  skills: reloadable(SkillsScreen),
  account: AccountScreen,
  global: GlobalSettingsScreen,
};

/** Écran affiché, piloté par SettingsView (forcé sur les fournisseurs sans profil). */
const activeScreen = ref<string | undefined>(undefined);

function onScreenChange(screenId: string) {
  activeScreen.value = screenId;
}

// --- Assistant de configuration (ContextChat, contexte `settings`) -----------

/** Nombre de profils de modèle : `null` tant que inconnu (chargement ou erreur). */
const profileCount = ref<number | null>(null);

/** Sans profil, l'assistant est désactivé et l'écran met en avant fournisseurs et profils. */
const assistantReady = computed(() => profileCount.value !== 0);

async function refreshProfiles() {
  const wasUnknown = profileCount.value === null;
  try {
    const rows = await listModelProfiles();
    profileCount.value = rows.length;
    if (rows.length === 0 && wasUnknown) {
      activeScreen.value = 'llm-providers';
      activeNav.value = { groupLabel: 'Modèles', screenLabel: 'Fournisseurs LLM' };
    }
  } catch {
    profileCount.value = null;
  }
}

function onNavChange(payload: { groupLabel: string; screenLabel: string }) {
  activeNav.value = { groupLabel: payload.groupLabel, screenLabel: payload.screenLabel };
  void refreshProfiles();
}

function onTurnFinished() {
  reloadKey.value += 1;
  void refreshProfiles();
}

const settingsChatData = {};

// --- Mise en page : panneau latéral (≥ md) ou onglets (< md) -----------------

const width = useViewportWidth();
const narrow = computed(() => width.value < MD_MIN_WIDTH);
const panelOpen = ref(true);
const mobileTab = ref<'config' | 'assistant'>('config');
</script>

<template>
  <div class="settings-view" :class="{ 'settings-view--narrow': narrow }">
    <div v-if="narrow" class="settings-tabs" role="tablist" aria-label="Configuration et assistant">
      <button
        type="button"
        role="tab"
        class="settings-tab"
        :aria-selected="mobileTab === 'config'"
        @click="mobileTab = 'config'"
      >
        Configuration
      </button>
      <button
        type="button"
        role="tab"
        class="settings-tab"
        :aria-selected="mobileTab === 'assistant'"
        @click="mobileTab = 'assistant'"
      >
        Assistant
      </button>
    </div>

    <div class="settings-main" v-show="!narrow || mobileTab === 'config'">
      <ConfigShell
        :groups="groups"
        :screens="screens"
        :active-screen="activeScreen"
        @update:active-screen="onScreenChange"
        @nav-change="onNavChange"
      />
    </div>

    <aside
      v-if="narrow ? mobileTab === 'assistant' : panelOpen"
      class="settings-assistant"
      :data-layout="narrow ? 'tab' : 'panel'"
      aria-label="Assistant de configuration"
    >
      <div v-if="!narrow" class="settings-assistant__bar">
        <button type="button" class="btn" @click="panelOpen = false">Replier l'assistant</button>
      </div>
      <p v-if="!assistantReady" class="settings-assistant__disabled" role="alert">
        Aucun profil de modèle : l'assistant n'est pas utilisable. Crée d'abord un fournisseur LLM
        puis un profil de modèle dans les formulaires de configuration, puis reviens ici.
      </p>
      <ContextChat
        v-else
        kind="settings"
        :data="settingsChatData"
        @turn-finished="onTurnFinished"
      />
    </aside>

    <button
      v-if="!narrow && !panelOpen"
      type="button"
      class="settings-assistant-open"
      @click="panelOpen = true"
    >
      Ouvrir l'assistant
    </button>
  </div>
</template>

<style scoped>
.settings-view {
  display: flex;
  min-height: 100%;
  width: 100%;
}

.settings-main {
  flex: 1;
  min-width: 0;
}

.settings-assistant {
  display: flex;
  flex-direction: column;
  width: 380px;
  flex-shrink: 0;
  min-width: 0;
  border-left: 1px solid #2b2b4a;
}

.settings-assistant__bar {
  display: flex;
  justify-content: flex-end;
  padding: 8px 12px;
}

.settings-assistant__disabled {
  padding: 16px;
  font-size: 13px;
}

.settings-assistant-open {
  position: fixed;
  right: 16px;
  bottom: 16px;
  min-height: 44px;
  padding: 0 16px;
}

.settings-tabs {
  display: flex;
  gap: 8px;
  padding: 8px 16px;
  width: 100%;
}

.settings-tab {
  flex: 1;
  min-height: 44px;
}

.settings-tab[aria-selected='true'] {
  font-weight: 600;
}

/* Sous md : l'onglet « Assistant » occupe tout l'écran. */
.settings-view--narrow .settings-assistant {
  width: 100%;
  border-left: 0;
}
</style>
