<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import type { RunDetail, RunState } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import DialogShell from '../common/DialogShell.vue';
import EmptyState from '../common/EmptyState.vue';
import ErrorCard from '../common/ErrorCard.vue';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';

/** États où le run est encore en cours côté moteur (`Unknown` = purgé, pas actif). */
const ACTIVE_STATES: readonly RunState[] = ['Pending', 'Running', 'Suspended', 'BackingOff'];

const props = withDefaults(
  defineProps<{ runId: string; client?: WorkflowsClient }>(),
  { client: () => createWorkflowsClient(createApiClient()) },
);

const run = ref<RunDetail | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

const isActive = computed(() =>
  run.value !== null && ACTIVE_STATES.includes(run.value.status.state),
);
const isPurged = computed(() => run.value?.status.state === 'Unknown');

function describeError(err: unknown): string {
  if (err instanceof EngineUnavailable) return 'Moteur de workflow indisponible.';
  return err instanceof ApiError ? err.message : String(err);
}

async function load() {
  loading.value = true;
  error.value = null;
  try {
    run.value = await props.client.getRun(props.runId);
  } catch (err) {
    error.value = describeError(err);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.runId, load);

// --- Annulation : confirmation obligatoire ----------------------------------

const cancelOpen = ref(false);
const cancelError = ref<string | null>(null);

function askCancel() {
  cancelError.value = null;
  cancelOpen.value = true;
}

async function confirmCancel() {
  cancelError.value = null;
  try {
    await props.client.cancelRun(props.runId);
    cancelOpen.value = false;
    await load();
  } catch (err) {
    cancelError.value = describeError(err);
  }
}
</script>

<template>
  <section class="run-detail">
    <LoadingSkeleton v-if="loading && !run" />
    <ErrorCard v-else-if="error" :message="error" />
    <div v-else-if="run">
      <header class="rd-header">
        <div>
          <h2>{{ run.workflow }}</h2>
          <div class="rd-meta">Run {{ run.run_id }} · {{ run.triggered_by }}</div>
        </div>
        <div class="rd-actions">
          <button type="button" class="btn" :disabled="loading" @click="load">Actualiser</button>
          <button v-if="isActive" type="button" class="btn btn-danger" @click="askCancel">
            Annuler le run
          </button>
        </div>
      </header>

      <div class="rd-state">
        <span class="rd-label">Statut</span>
        <span class="rd-value">{{ run.status.state }}</span>
      </div>
      <p v-if="isPurged" class="rd-purged">
        Ce run a été purgé du moteur : son état n'est plus disponible.
      </p>
      <p v-if="run.status.error" class="rd-error">{{ run.status.error }}</p>

      <h3>Steps</h3>
      <EmptyState v-if="run.status.steps.length === 0" message="Aucun step." />
      <table v-else class="steps-table">
        <thead>
          <tr>
            <th>Step</th>
            <th>Statut</th>
            <th>Erreur</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in run.status.steps" :key="s.step_id">
            <td>{{ s.step_id }}</td>
            <td>{{ s.state }}</td>
            <td class="step-error">{{ s.error ?? '—' }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <DialogShell v-model:open="cancelOpen" title="Annuler le run ?">
      <p class="confirm-text">
        Le run sera interrompu. Cette action ne peut pas être annulée.
      </p>
      <div v-if="cancelError" class="confirm-error">{{ cancelError }}</div>
      <template #actions>
        <button type="button" class="btn btn-danger" @click="confirmCancel">
          Confirmer l'annulation
        </button>
        <button type="button" class="btn" @click="cancelOpen = false">Retour</button>
      </template>
    </DialogShell>
  </section>
</template>

<style scoped>
/* Région de contenu de l'écran : elle défile elle-même (la coquille ne défile pas). */
.run-detail {
  box-sizing: border-box;
  height: 100%;
  overflow-y: auto;
  color: #e6e9f0;
  max-width: 960px;
}

h2 {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
  word-break: break-word;
}

h3 {
  font-size: 14px;
  font-weight: 600;
  margin: 20px 0 8px;
}

.rd-header {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  justify-content: space-between;
  align-items: flex-start;
  margin-bottom: 16px;
}

.rd-meta {
  font-size: 12px;
  color: #6a7185;
  margin-top: 4px;
}

.rd-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.rd-state {
  display: flex;
  gap: 12px;
  align-items: baseline;
}

.rd-label {
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}

.rd-value {
  font-weight: 600;
}

.rd-purged {
  color: #6a7185;
  font-size: 13px;
  margin: 8px 0 0;
}

.rd-error {
  color: #e85d5d;
  font-size: 13px;
  margin: 8px 0 0;
  word-break: break-word;
}

.steps-table {
  width: 100%;
  border-collapse: collapse;
}

.steps-table th,
.steps-table td {
  text-align: left;
  padding: 8px 10px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
  vertical-align: top;
  word-break: break-word;
}

.steps-table th {
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}

.step-error {
  color: #e85d5d;
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

.btn:disabled {
  color: #6a7185;
  cursor: not-allowed;
}

.btn-danger {
  background: #e85d5d22;
  border-color: #e85d5d44;
  color: #e85d5d;
}

.confirm-text {
  font-size: 13px;
  margin: 0 0 12px;
}

.confirm-error {
  color: #e85d5d;
  font-size: 13px;
  margin-bottom: 8px;
}
</style>
