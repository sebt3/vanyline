<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { RouterLink, useRoute } from 'vue-router';
import type { CheckStatus, HealthState } from '@vanyline/protocol';
import { createApiClient } from '../api/client';
import { createWorkflowsClient } from '../api/workflowsClient';
import AppBreadcrumb from './AppBreadcrumb.vue';

defineProps<{ workspace?: string; extended?: boolean }>();

/** Intervalles d'interrogation de la santé du code (ide.sdd, sandbox-overview.sdd). */
const POLL_BUSY_MS = 10_000;
const POLL_IDLE_MS = 30_000;

const route = useRoute();
const workflows = createWorkflowsClient(createApiClient());

/** Sandbox et projet de la route IDE ; `null` hors IDE (pas d'indicateur). */
const sandboxName = computed<string | null>(() =>
  route.name === 'ide' && typeof route.params.sandboxName === 'string'
    ? route.params.sandboxName
    : null,
);
const projectName = computed<string>(() =>
  typeof route.params.projectName === 'string' ? route.params.projectName : '',
);

/** Vue d'ensemble de la même sandbox (chemin, pas nom de route : la barre d'état ne dépend pas du routeur complet). */
const overviewPath = computed<string>(
  () =>
    `/p/${encodeURIComponent(projectName.value)}/s/${encodeURIComponent(sandboxName.value ?? '')}/overview`,
);

const health = ref<HealthState | null>(null);
const healthError = ref<string | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined;

const statusLabels: Record<CheckStatus, string> = {
  passed: 'réussi',
  failed: 'échec',
  errored: 'erreur',
  skipped: 'ignoré',
};

/** Statut global lu tel quel depuis `summary` (dérivé par la sandbox, jamais recalculé). */
const statusText = computed<string | null>(() => {
  const status = health.value?.summary?.status;
  return status === undefined ? null : statusLabels[status];
});

const coverageText = computed<string | null>(() => {
  const percent = health.value?.summary?.coverage_percent;
  if (percent === undefined) return null;
  if (percent === null) return 'couverture n/d';
  return `${Number.isInteger(percent) ? percent : percent.toFixed(1)} %`;
});

const freshness = computed<{ label: string; tone: string } | null>(() => {
  switch (health.value?.phase) {
    case 'computing':
      return { label: 'calcul en cours', tone: 'busy' };
    case 'stale':
      return { label: 'périmé', tone: 'warn' };
    case 'fresh':
      return { label: 'à jour', tone: 'ok' };
    default:
      return null;
  }
});

function pollDelay(): number {
  return health.value?.phase === 'fresh' ? POLL_IDLE_MS : POLL_BUSY_MS;
}

function stopPolling() {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
}

function schedulePoll() {
  stopPolling();
  if (sandboxName.value === null || document.visibilityState === 'hidden') return;
  timer = setTimeout(() => void loadHealth(), pollDelay());
}

async function loadHealth() {
  stopPolling();
  const sandbox = sandboxName.value;
  if (sandbox === null) return;
  try {
    const state = await workflows.getCodeHealth(sandbox);
    if (sandboxName.value === sandbox) {
      health.value = state;
      healthError.value = null;
    }
  } catch (err) {
    healthError.value = err instanceof Error ? err.message : String(err);
  } finally {
    schedulePoll();
  }
}

function onVisibilityChange() {
  if (document.visibilityState === 'hidden') {
    stopPolling();
  } else {
    void loadHealth();
  }
}

watch(sandboxName, () => {
  health.value = null;
  healthError.value = null;
  void loadHealth();
});

onMounted(() => {
  void loadHealth();
  document.addEventListener('visibilitychange', onVisibilityChange);
});

onBeforeUnmount(() => {
  stopPolling();
  document.removeEventListener('visibilitychange', onVisibilityChange);
});
</script>

<template>
  <div class="statusbar">
    <AppBreadcrumb />
    <span class="grow" />
    <template v-if="extended">
      <RouterLink
        v-if="sandboxName !== null"
        data-testid="health-indicator"
        class="item health"
        :to="overviewPath"
        title="Santé du code : ouvrir la vue de sandbox"
      >
        <span class="health-label">santé</span>
        <span v-if="healthError" class="health-error">indisponible</span>
        <template v-else-if="health">
          <span v-if="statusText" class="health-status">{{ statusText }}</span>
          <span v-if="coverageText" class="health-coverage">{{ coverageText }}</span>
          <span v-if="freshness" class="health-fresh" :class="freshness.tone">{{ freshness.label }}</span>
        </template>
        <span v-else class="health-fresh busy">chargement…</span>
      </RouterLink>
      <span class="item">⎇ main</span>
      <span class="item">{{ workspace }}</span>
      <span class="item">● connecté — strix-cluster</span>
      <span class="item">UTF-8</span>
    </template>
  </div>
</template>

<style scoped>
.statusbar {
  height: 22px;
  flex: none;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 0 10px;
  background: var(--dv-color-abyss-accent);
  color: white;
  font-size: 11px;
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
}
.grow { flex: 1; }
.item { opacity: 0.92; }
.health {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: white;
  text-decoration: none;
}
.health:hover { text-decoration: underline; }
.health-label { opacity: 0.75; }
.health-status { font-weight: 600; }
.health-fresh.busy { color: #e0a83d; }
.health-fresh.warn { color: #e0a83d; }
.health-fresh.ok { color: #3fb56d; }
.health-error { color: #ffb4c8; }
</style>
