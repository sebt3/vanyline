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
import AssistantPanel from './shell/AssistantPanel.vue';
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
    accent: 'var(--accent)',
    sub: [
      { id: 'llm-providers', label: 'Fournisseurs LLM' },
      { id: 'model-profiles', label: 'Profils de modèle' },
    ],
  },
  {
    id: 'outils',
    label: 'Outils',
    icon: '⚙',
    accent: 'var(--accent)',
    sub: [
      { id: 'mcp-servers', label: 'Serveurs MCPs' },
      { id: 'toolsets', label: 'Toolsets' },
    ],
  },
  { id: 'agents', label: 'Agents', icon: '✦', accent: 'var(--accent)' },
  { id: 'skills', label: 'Skills', icon: '⚡', accent: 'var(--accent)' },
  { id: 'account', label: 'Compte', icon: '●', accent: 'var(--accent)' },
];

/** Groupe « Global » (modèle par défaut, app-settings) : administrateurs seulement. */
const globalGroup: ConfigNavGroup = { id: 'global', label: 'Global', icon: '◎', accent: 'var(--accent)' };

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
    <header class="settings-head">
      <div v-if="narrow" class="settings-seg" role="tablist" aria-label="Configuration et assistant">
        <button
          type="button"
          role="tab"
          class="settings-seg__tab"
          :aria-selected="mobileTab === 'config'"
          @click="mobileTab = 'config'"
        >
          Configuration
        </button>
        <button
          type="button"
          role="tab"
          class="settings-seg__tab"
          :aria-selected="mobileTab === 'assistant'"
          @click="mobileTab = 'assistant'"
        >
          Assistant
        </button>
      </div>
      <button
        v-else
        type="button"
        class="settings-head__toggle"
        :aria-expanded="panelOpen"
        @click="panelOpen = !panelOpen"
      >
        Assistant
      </button>
    </header>

    <div class="settings-body">
      <div class="settings-main" v-show="!narrow || mobileTab === 'config'">
        <ConfigShell
          :groups="groups"
          :screens="screens"
          :active-screen="activeScreen"
          @update:active-screen="onScreenChange"
          @nav-change="onNavChange"
        />
      </div>

      <AssistantPanel
        v-if="!narrow"
        :open="panelOpen"
        title="Assistant de configuration"
        @close="panelOpen = false"
      >
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
      </AssistantPanel>

      <section
        v-else-if="mobileTab === 'assistant'"
        class="settings-assistant"
        data-layout="tab"
        aria-label="Assistant de configuration"
      >
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
      </section>
    </div>
  </div>
</template>

<style scoped>
/* Contrat : ./SettingsView.sdd (refonte 2026-10-21) et ./shell/shell.sdd. La page ne défile
   jamais : seules la navigation de configuration et la liste de messages défilent (layout.sdd).
   Couleurs uniquement via les jetons de theme.css ; hauteurs de contrôle via --ctl-h. */
.settings-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  width: 100%;
  overflow: hidden;
  background: var(--bg);
  color: var(--text);
  font-family: var(--font-sans);
}

.settings-head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 4px 12px;
  border-bottom: 1px solid var(--border);
}

.settings-head__toggle,
.settings-seg__tab {
  min-height: var(--ctl-h);
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
  cursor: pointer;
}

.settings-head__toggle[aria-expanded='true'] {
  background: var(--surface-2);
}

.settings-seg {
  flex: 1;
  display: flex;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--surface);
}

.settings-seg__tab {
  flex: 1;
  border: 0;
  border-radius: 0;
}

.settings-seg__tab[aria-selected='true'] {
  background: var(--surface-2);
  font-weight: 600;
}

.settings-body {
  flex: 1;
  display: flex;
  min-height: 0;
  min-width: 0;
}

.settings-main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
}

.settings-assistant {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.settings-assistant__disabled {
  margin: 0;
  padding: 16px;
  font-size: 13px;
  color: var(--muted);
}

/* Sous md : le choix Configuration / Assistant est sous l'en-tête ; une seule région à la fois. */
.settings-view--narrow .settings-body {
  flex-direction: column;
}

.settings-view--narrow .settings-assistant {
  background: var(--bg);
}
</style>
