<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import type { WorkflowScope, WorkflowSummary } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import DialogShell from '../common/DialogShell.vue';
import EmptyState from '../common/EmptyState.vue';
import ErrorCard from '../common/ErrorCard.vue';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';

const props = withDefaults(
  defineProps<{
    project: string;
    client?: WorkflowsClient;
    /** Lecture seule : masque création, édition et suppression (le lancement reste). */
    readonly?: boolean;
    /** Portée imposée par l'appelant : la liste ne montre que celle-ci, sans filtre. */
    fixedScope?: WorkflowScope;
  }>(),
  { client: () => createWorkflowsClient(createApiClient()), readonly: false, fixedScope: undefined },
);

const emit = defineEmits<{
  create: [];
  edit: [id: number];
  launch: [workflow: WorkflowSummary];
  delete: [id: number];
}>();

const workflows = ref<WorkflowSummary[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const engineUnavailable = ref(false);

/** Filtre de portée ; `all` = pas de filtre. */
const scopeFilter = ref<'all' | WorkflowScope>('all');

const visible = computed(() => {
  const scope = props.fixedScope ?? (scopeFilter.value === 'all' ? undefined : scopeFilter.value);
  return scope === undefined ? workflows.value : workflows.value.filter((w) => w.scope === scope);
});

const scopeFilters: { value: 'all' | WorkflowScope; label: string }[] = [
  { value: 'all', label: 'Toutes' },
  { value: 'project', label: 'Projet' },
  { value: 'sandbox', label: 'Sandbox' },
];

function scopeLabel(scope: WorkflowScope): string {
  return scope === 'project' ? 'projet' : 'sandbox';
}

function describeError(err: unknown): string {
  if (err instanceof EngineUnavailable) {
    return 'Moteur de workflow indisponible.';
  }
  return err instanceof ApiError ? err.message : String(err);
}

async function load() {
  loading.value = true;
  error.value = null;
  engineUnavailable.value = false;
  try {
    workflows.value = await props.client.listWorkflows(props.project);
  } catch (err) {
    engineUnavailable.value = err instanceof EngineUnavailable;
    error.value = describeError(err);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.project, load);

// --- Suppression : confirmation obligatoire ---------------------------------

const deleteOpen = ref(false);
const pendingDelete = ref<WorkflowSummary | null>(null);
const deleteError = ref<string | null>(null);

function askDelete(wf: WorkflowSummary) {
  pendingDelete.value = wf;
  deleteError.value = null;
  deleteOpen.value = true;
}

async function confirmDelete() {
  const wf = pendingDelete.value;
  if (!wf) return;
  deleteError.value = null;
  try {
    await props.client.deleteWorkflow(props.project, wf.id);
    deleteOpen.value = false;
    pendingDelete.value = null;
    emit('delete', wf.id);
    await load();
  } catch (err) {
    deleteError.value = describeError(err);
  }
}
</script>

<template>
  <section class="workflow-list">
    <header class="wf-header">
      <h2>Workflows</h2>
      <button v-if="!readonly" type="button" class="btn btn-primary" @click="emit('create')">
        Nouveau workflow
      </button>
    </header>

    <div v-if="fixedScope === undefined" class="scope-filter" role="group" aria-label="Filtrer par portée">
      <button
        v-for="f in scopeFilters"
        :key="f.value"
        type="button"
        class="btn btn-filter"
        :class="{ 'btn-filter--active': scopeFilter === f.value }"
        :aria-pressed="scopeFilter === f.value"
        @click="scopeFilter = f.value"
      >
        {{ f.label }}
      </button>
    </div>

    <LoadingSkeleton v-if="loading" />
    <div v-else>
      <ErrorCard v-if="error" :message="error" />
      <EmptyState
        v-else-if="visible.length === 0"
        message="Aucun workflow."
      />
      <template v-else>
        <!-- Tableau à partir de md (768 px), cartes en dessous. -->
        <div class="wf-table-wrap">
          <table class="wf-table">
            <thead>
              <tr>
                <th>Nom</th>
                <th>Portée</th>
                <th>Kinds</th>
                <th>Steps</th>
                <th class="th-actions">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="wf in visible" :key="wf.id">
                <td>{{ wf.name }}</td>
                <td>{{ scopeLabel(wf.scope) }}</td>
                <td>{{ wf.kinds.join(', ') || '—' }}</td>
                <td>{{ wf.steps_count }}</td>
                <td class="td-actions">
                  <button type="button" class="btn btn-launch" @click="emit('launch', wf)">
                    Lancer
                  </button>
                  <template v-if="!readonly">
                    <button type="button" class="btn" @click="emit('edit', wf.id)">
                      Éditer
                    </button>
                    <button type="button" class="btn btn-danger" @click="askDelete(wf)">
                      Supprimer
                    </button>
                  </template>
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <ul class="wf-cards">
          <li v-for="wf in visible" :key="wf.id" class="wf-card">
            <div class="wf-card-title">{{ wf.name }}</div>
            <div class="wf-card-meta">
              Portée : {{ scopeLabel(wf.scope) }} · Steps : {{ wf.steps_count }}
            </div>
            <div class="wf-card-meta">Kinds : {{ wf.kinds.join(', ') || '—' }}</div>
            <div class="wf-card-actions">
              <button type="button" class="btn btn-launch" @click="emit('launch', wf)">
                Lancer
              </button>
              <template v-if="!readonly">
                <button type="button" class="btn" @click="emit('edit', wf.id)">
                  Éditer
                </button>
                <button type="button" class="btn btn-danger" @click="askDelete(wf)">
                  Supprimer
                </button>
              </template>
            </div>
          </li>
        </ul>
      </template>
    </div>

    <DialogShell v-model:open="deleteOpen" title="Supprimer le workflow ?">
      <p class="confirm-text">
        Le workflow « {{ pendingDelete?.name }} » sera supprimé. Cette action est définitive.
      </p>
      <div v-if="deleteError" class="confirm-error">{{ deleteError }}</div>
      <template #actions>
        <button type="button" class="btn btn-danger" @click="confirmDelete">Supprimer</button>
        <button type="button" class="btn" @click="deleteOpen = false">Annuler</button>
      </template>
    </DialogShell>
  </section>
</template>

<style scoped>
.workflow-list {
  color: #e6e9f0;
  max-width: 960px;
}

.wf-header {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
}

h2 {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
}

.scope-filter {
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
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

.btn-primary,
.btn-launch {
  background: #4c90f0;
  border-color: #4c90f0;
  color: white;
}

.btn-primary:hover,
.btn-launch:hover {
  background: #3a7de0;
}

.btn-danger {
  background: #e85d5d22;
  border-color: #e85d5d44;
  color: #e85d5d;
}

.btn-filter--active {
  background: #2b3550;
  border-color: #4c90f0;
}

.wf-table-wrap {
  display: none;
}

.wf-table {
  width: 100%;
  border-collapse: collapse;
}

.wf-table th,
.wf-table td {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
}

.wf-table th {
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}

.td-actions,
.th-actions {
  text-align: right;
}

.td-actions .btn {
  margin-left: 6px;
}

.wf-cards {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 12px;
}

.wf-card {
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  padding: 14px 16px;
}

.wf-card-title {
  font-weight: 600;
  margin-bottom: 6px;
}

.wf-card-meta {
  font-size: 13px;
  color: #6a7185;
  margin-bottom: 4px;
}

.wf-card-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 10px;
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

@media (min-width: 768px) {
  .wf-table-wrap {
    display: block;
  }

  .wf-cards {
    display: none;
  }
}
</style>
