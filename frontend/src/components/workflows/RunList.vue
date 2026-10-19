<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import type { RunState, RunSummary } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import EmptyState from '../common/EmptyState.vue';
import ErrorCard from '../common/ErrorCard.vue';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';

/** Fenêtre des runs affichés : 14 jours, 50 au plus (défauts de l'API). */
const WINDOW = { limit: 50, since: 14 } as const;
/** Intervalle d'interrogation tant qu'un run est actif. */
const POLL_MS = 10_000;
/** États où le run est encore en cours côté moteur (`Unknown` = purgé, pas actif). */
const ACTIVE_STATES: readonly RunState[] = ['Pending', 'Running', 'Suspended', 'BackingOff'];

const props = withDefaults(
  defineProps<{ project?: string; sandbox?: string; client?: WorkflowsClient }>(),
  { project: undefined, sandbox: undefined, client: () => createWorkflowsClient(createApiClient()) },
);

const emit = defineEmits<{ select: [runId: string] }>();

const runs = ref<RunSummary[]>([]);
const loaded = ref(false);
const loading = ref(false);
const error = ref<string | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined;

function isActive(r: RunSummary): boolean {
  return ACTIVE_STATES.includes(r.status.state);
}

function stopPolling() {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
}

function schedulePoll() {
  stopPolling();
  if (runs.value.some(isActive)) {
    timer = setTimeout(() => void load(), POLL_MS);
  }
}

function describeError(err: unknown): string {
  if (err instanceof EngineUnavailable) return 'Moteur de workflow indisponible.';
  return err instanceof ApiError ? err.message : String(err);
}

async function load() {
  stopPolling();
  // Le premier chargement affiche le squelette ; les interrogations suivantes non.
  if (!loaded.value) loading.value = true;
  error.value = null;
  try {
    runs.value = props.sandbox !== undefined
      ? await props.client.listSandboxRuns(props.sandbox, WINDOW)
      : await props.client.listProjectRuns(props.project ?? '', WINDOW);
  } catch (err) {
    error.value = describeError(err);
  } finally {
    loading.value = false;
    loaded.value = true;
    schedulePoll();
  }
}

onMounted(load);
onUnmounted(stopPolling);
watch(() => [props.project, props.sandbox], () => {
  loaded.value = false;
  void load();
});

function formatDate(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString('fr-FR');
}
</script>

<template>
  <section class="run-list">
    <h2>Runs</h2>

    <LoadingSkeleton v-if="loading" />
    <div v-else>
      <ErrorCard v-if="error" :message="error" />
      <EmptyState v-else-if="runs.length === 0" message="Aucun run sur les 14 derniers jours." />
      <template v-else>
        <!-- Tableau à partir de md (768 px), cartes en dessous. -->
        <div class="run-table-wrap">
          <table class="run-table">
            <thead>
              <tr>
                <th>Workflow</th>
                <th>Déclencheur</th>
                <th>Heure</th>
                <th>Statut</th>
                <th class="th-actions"></th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="r in runs" :key="r.run_id">
                <td>{{ r.workflow }}</td>
                <td>{{ r.triggered_by }}</td>
                <td>{{ formatDate(r.started_at) }}</td>
                <td>{{ r.status.state }}</td>
                <td class="td-actions">
                  <button type="button" class="btn" @click="emit('select', r.run_id)">
                    Détail
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <ul class="run-cards">
          <li v-for="r in runs" :key="r.run_id" class="run-card">
            <div class="run-card-title">{{ r.workflow }}</div>
            <div class="run-card-meta">
              {{ r.status.state }} · {{ r.triggered_by }} · {{ formatDate(r.started_at) }}
            </div>
            <button type="button" class="btn run-card-btn" @click="emit('select', r.run_id)">
              Détail
            </button>
          </li>
        </ul>
      </template>
    </div>
  </section>
</template>

<style scoped>
.run-list {
  color: #e6e9f0;
  max-width: 960px;
}

h2 {
  font-size: 16px;
  font-weight: 600;
  margin: 0 0 16px;
}

.btn {
  min-height: var(--ctl-h);
  padding: 0 14px;
  border-radius: 6px;
  border: 1px solid #2b2b4a;
  background: #1c1c2a;
  color: #e6e9f0;
  cursor: pointer;
  font-size: 13px;
}

.btn:hover {
  background: #2b2b4a;
}

.run-table-wrap {
  display: none;
}

.run-table {
  width: 100%;
  border-collapse: collapse;
}

.run-table th,
.run-table td {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
}

.run-table th {
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}

.td-actions,
.th-actions {
  text-align: right;
}

.run-cards {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 12px;
}

.run-card {
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  padding: 14px 16px;
}

.run-card-title {
  font-weight: 600;
  margin-bottom: 6px;
  word-break: break-word;
}

.run-card-meta {
  font-size: 13px;
  color: #6a7185;
  margin-bottom: 10px;
}

@media (min-width: 768px) {
  .run-table-wrap {
    display: block;
  }

  .run-cards {
    display: none;
  }
}
</style>
