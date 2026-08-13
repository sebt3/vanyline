<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import {
  DialogRoot, DialogPortal, DialogContent, DialogTitle, DialogClose,
} from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';

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

const props = defineProps<{ projectName: string }>();

const client = createApiClient();
const router = useRouter();

/** Sandboxes du projet courant, filtrées côté client (fetch global /api/sandboxes). */
const projectSandboxes = computed(() =>
  fetchedSandboxes.value.filter((s) => s.spec.project === props.projectName),
);

function openSandbox(name: string) {
  router.push(`/p/${props.projectName}/s/${name}`);
}

const fetchedSandboxes = ref<Sandbox[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const formName = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

async function fetchSandboxes() {
  try {
    fetchedSandboxes.value = await client.get<Sandbox[]>('/api/sandboxes');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(fetchSandboxes);

const modalOpen = ref(false);

async function createSandbox() {
  creationError.value = null;
  const body: CreateSandboxBody = {
    name: formName.value,
    project: props.projectName,
    branch: formBranch.value,
  };
  try {
    await client.post<Sandbox>('/api/sandboxes', body);
    formName.value = '';
    formBranch.value = '';
    modalOpen.value = false;
    await fetchSandboxes();
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
    await fetchSandboxes();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteSandbox(name: string) {
  try {
    await client.delete(`/api/sandboxes/${name}`);
    await fetchSandboxes();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}
</script>

<template>
  <div class="dashboard">
    <h1>Sandboxes de {{ projectName }}</h1>
    <div class="actions-row">
      <button class="btn btn-create" @click="modalOpen = true">Créer une sandbox</button>
      <button class="btn btn-back" @click="router.push('/')">Retour</button>
      <button class="btn btn-settings" @click="router.push('/settings')">Paramètres</button>
    </div>

    <div v-if="loading" class="card">
      <div class="skeleton" />
      <div class="skeleton short" />
      <div class="skeleton short" />
    </div>
    <div v-else>
      <div class="card" v-if="error" role="alert">
        <p class="error-text">{{ error }}</p>
      </div>
      <div v-else>
        <div class="card" v-if="projectSandboxes.length === 0">Aucune sandbox.</div>
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
                @click="openSandbox(s.metadata.name)" class="row-clickable">
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
                <button class="btn btn-open" @click.stop="openSandbox(s.metadata.name)">Ouvrir</button>
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

      <DialogRoot v-model:open="modalOpen">
        <DialogPortal>
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Créer une sandbox</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="formName"
                type="text"
                placeholder="ma-sandbox"
                aria-label="Nom de la sandbox"
              />
            </label>
            <label class="field">
              <span class="field-label">Branche</span>
              <input
                class="field-input"
                v-model="formBranch"
                type="text"
                placeholder="main"
                aria-label="Branche"
              />
            </label>
            <div v-if="creationError" class="creation-error">{{ creationError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-create" @click="createSandbox">Créer</button>
              <DialogClose class="btn btn-cancel">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>
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

.card {
  display: grid;
  grid-template-columns: 1fr;
  gap: 22px 28px;
  max-width: 760px;
  padding: 28px 32px;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  margin-bottom: 12px;
}

.card .error-text {
  color: #e85d5d;
  font-size: 13px;
  margin: 0;
}

.skeleton {
  height: 16px;
  border-radius: 4px;
  background: linear-gradient(90deg, #1a2332 25%, #1f2b3d 50%, #1a2332 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}
.skeleton.short {
  width: 60%;
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

.btn {
  appearance: none;
  border: none;
  font: inherit;
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 6px;
  cursor: pointer;
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

.btn-delete {
  background: #5b1e3f22;
  color: #e85d5d;
  border: 1px solid #e85d5d44;
}

.btn-delete:hover {
  background: #e85d5d33;
}

.btn-open {
  background: #4c90f0;
  color: white;
  margin-right: 6px;
}

.btn-open:hover {
  background: #3a7de0;
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

.btn-cancel {
  background: #1c1c2a;
  color: #6a7185;
  border: 1px solid #2b2b4a;
}

.btn-cancel:hover {
  background: #2b2b4a;
  color: #e6e9f0;
}

.btn-create {
  background: #4c90f0;
  color: white;
  font-weight: 600;
  padding: 6px 16px;
}

.btn-create:hover {
  background: #3a7de0;
}

.field {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 8px 12px;
  align-items: center;
  margin-bottom: 12px;
}

.field-label {
  font-size: 12px;
  font-weight: 600;
  color: #6a7185;
  text-transform: uppercase;
}

.field-input {
  width: 100%;
  padding: 6px 10px;
  background: #0c1420;
  border: 1px solid #1c1c2a;
  border-radius: 6px;
  color: #e6e9f0;
  font: inherit;
  font-size: 13px;
}

.field-input:focus {
  outline: none;
  border-color: #4c90f0;
}

.creation-error {
  color: #e85d5d;
  font-size: 12px;
  margin-top: 4px;
  margin-bottom: 12px;
}
</style>

<style>
[role='dialog'] {
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  padding: 24px 28px;
  max-width: 480px;
}

.dialog-title {
  margin: 0 0 16px 0;
  font-size: 15px;
  font-weight: 600;
  color: #e6e9f0;
}

.dialog-actions {
  display: flex;
  gap: 12px;
  justify-content: flex-end;
  margin-top: 12px;
}
</style>