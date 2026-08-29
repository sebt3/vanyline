<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import { useRouter } from 'vue-router';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import { useSandboxState, registerRefresh, unregisterRefresh } from '../../composables/useSandboxState';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';

interface Toolchain {
  name: string;
  image: string;
}

interface SandboxSpec {
  project: string;
  branch: string;
  toolchains?: Toolchain[];
  resources?: unknown;
  suspended?: boolean;
}

interface Sandbox {
  metadata: { name: string };
  spec: SandboxSpec;
  status?: { phase?: string | null };
}

interface CreateSandboxBody {
  name: string;
  project: string;
  branch: string;
}

interface Project {
  metadata: { name: string };
  status?: { cloned?: boolean } | null;
}

const props = defineProps<{ projectName: string }>();

const client = createApiClient();
const router = useRouter();
const resource = useCrudResource<Sandbox>(client, '/api/sandboxes');
const { items: fetchedSandboxes, loading, error } = resource;
useSandboxState();

/** Le clone initial du Project doit être terminé avant de pouvoir créer une
 *  sandbox dessus (le worktree n'existe pas encore sinon). `null` tant que
 *  le Project n'est pas chargé — traité comme "pas prêt" (bouton désactivé
 *  par défaut, jamais un flash de faux-positif). */
const project = ref<Project | null>(null);
const projectReady = computed(() => project.value?.status?.cloned === true);

onMounted(() => {
  resource.fetch();
  // Le hub sandbox-state (WS temps réel) refetch le listing sur tout changement
  // de phase — cf. `useSandboxState`.
  registerRefresh(resource.fetch);
  fetchProject();
});

onBeforeUnmount(() => {
  unregisterRefresh(resource.fetch);
});

async function fetchProject() {
  try {
    project.value = await client.get<Project>(`/api/projects/${props.projectName}`);
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

/** Sandboxes du projet courant, filtrées côté client (fetch global /api/sandboxes). */
const projectSandboxes = computed(() =>
  fetchedSandboxes.value.filter((s) => s.spec.project === props.projectName),
);

function openSandbox(sandbox: Sandbox) {
  if (sandbox.status?.phase !== 'Running') return;
  router.push(`/p/${props.projectName}/s/${sandbox.metadata.name}`);
}

const formName = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

const modalOpen = ref(false);

async function createSandbox() {
  if (!projectReady.value) return;
  creationError.value = null;
  const body: CreateSandboxBody = {
    name: formName.value,
    project: props.projectName,
    branch: formBranch.value.trim() || 'main',
  };
  try {
    await resource.create(body);
    formName.value = '';
    formBranch.value = '';
    modalOpen.value = false;
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function suspendSandbox(name: string) {
  const sandbox = fetchedSandboxes.value.find((s) => s.metadata.name === name);
  if (!sandbox) return;
  const payload = { suspended: !sandbox.spec.suspended };
  try {
    await client.post<Sandbox>(`/api/sandboxes/${name}/suspend`, payload);
    await resource.fetch();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteSandbox(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <div class="dashboard">
    <h1>Sandboxes de {{ projectName }}</h1>
    <div class="actions-row">
      <button
        class="btn btn-create"
        :disabled="!projectReady"
        :title="projectReady ? undefined : 'Le clone initial du projet n\'est pas encore terminé'"
        @click="modalOpen = true"
      >
        Créer une sandbox
      </button>
      <button class="btn btn-back" @click="router.push('/')">Retour</button>
      <button class="btn btn-settings" @click="router.push('/settings')">Paramètres</button>
    </div>
    <p v-if="project && !projectReady" class="not-ready-hint">
      Clone du projet en cours — la création de sandbox sera disponible une fois terminé.
    </p>

    <LoadingSkeleton v-if="loading" />
    <div v-else>
      <ErrorCard v-if="error" :message="error" />
      <div v-else>
        <EmptyState v-if="projectSandboxes.length === 0" message="Aucune sandbox." />
        <table class="table" v-else>
          <thead>
            <tr>
              <th class="th-name">Nom</th>
              <th class="th-branch">Branche</th>
              <th class="th-phase">Phase</th>
              <th class="th-toolchains">Toolchains</th>
              <th class="th-action"></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="s in projectSandboxes" :key="s.metadata.name"
                @click="openSandbox(s)"
                :class="{ 'row-clickable': s.status?.phase === 'Running' }">
              <td>{{ s.metadata.name }}</td>
              <td>{{ s.spec.branch }}</td>
              <td>{{ s.status?.phase ?? '—' }}</td>
              <td>
                {{
                  s.spec.toolchains && s.spec.toolchains.length > 0
                    ? s.spec.toolchains.map((t) => t.name).join(', ')
                    : '—'
                }}
              </td>
              <td>
                <button
                  class="btn btn-open"
                  :disabled="s.status?.phase !== 'Running'"
                  :title="s.status?.phase !== 'Running' ? 'La sandbox n\'est pas encore prête' : undefined"
                  @click.stop="openSandbox(s)"
                >
                  Ouvrir
                </button>
                <button
                  class="btn btn-suspend"
                  :class="{ 'btn-suspended': s.spec.suspended }"
                  @click.stop="suspendSandbox(s.metadata.name)"
                >
                  {{ s.spec.suspended ? 'Reprendre' : 'Suspendre' }}
                </button>
                <button class="btn btn-delete" @click.stop="deleteSandbox(s.metadata.name)">
                  Supprimer
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <DialogShell v-model:open="modalOpen" title="Créer une sandbox">
        <Field label="Nom">
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="ma-sandbox"
            aria-label="Nom de la sandbox"
          />
        </Field>
        <Field label="Branche">
          <input
            class="field-input"
            v-model="formBranch"
            type="text"
            placeholder="main"
            aria-label="Branche"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" :disabled="!projectReady" @click="createSandbox">
            Créer
          </button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>
    </div>
  </div>
</template>

<style scoped>
.dashboard {
  background: #0c1420;
  color: #e6e9f0;
  padding: 48px 56px;
}

h1 {
  font-size: 18px;
  font-weight: 600;
  margin: 0 0 16px 0;
}

.actions-row {
  display: flex;
  gap: 12px;
  margin-bottom: 24px;
}

.table {
  width: 100%;
  max-width: 760px;
  border-collapse: collapse;
  margin-bottom: 24px;
}

.row-clickable {
  cursor: pointer;
}

.th-name,
.th-branch,
.th-phase,
.th-toolchains,
.th-action {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}
.th-name { width: 20%; }
.th-branch { width: 15%; }
.th-phase { width: 15%; }
.th-toolchains { width: 20%; }
.th-action { text-align: right; width: 15%; }

.table td {
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
}

.btn-suspend {
  background: #e0a83d22;
  color: #e0a83d;
  border: 1px solid #e0a83d44;
  margin-right: 6px;
}

.btn-suspend:hover {
  background: #e0a83d33;
}

.btn-suspended {
  background: #3fb56d22;
  color: #3fb56d;
  border: 1px solid #3fb56d44;
}

.btn-suspended:hover {
  background: #3fb56d33;
}

.btn-open {
  background: #4c90f0;
  color: white;
  margin-right: 6px;
}

.btn-open:hover {
  background: #3a7de0;
}

.btn-open:disabled {
  background: #2b3550;
  color: #6a7185;
  cursor: not-allowed;
}

.btn-open:disabled:hover {
  background: #2b3550;
}

.btn-back {
  background: #1c1c2a;
  color: #e6e9f0;
  border: 1px solid #2b2b4a;
}

.btn-back:hover {
  background: #2b2b4a;
}

.btn-settings {
  background: #1c1c2a;
  color: #e6e9f0;
  border: 1px solid #2b2b4a;
}

.btn-settings:hover {
  background: #2b2b4a;
}

.btn-create:disabled {
  background: #2b3550;
  color: #6a7185;
  cursor: not-allowed;
}

.btn-create:disabled:hover {
  background: #2b3550;
}

.not-ready-hint {
  color: #6a7185;
  font-size: 13px;
  margin: -12px 0 20px;
}
</style>
