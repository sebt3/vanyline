<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
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

const client = createApiClient();
const router = useRouter();

/** Ouvre l'IDE sur la sandbox choisie. La route /ide/:sandboxName est définie
 *  dans router.ts (task-02). */
function openSandbox(name: string) {
  router.push(`/ide/${name}`);
}

const fetchedSandboxes = ref<Sandbox[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const formName = ref('');
const formProject = ref('');
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

async function createSandbox() {
  creationError.value = null;
  const body: CreateSandboxBody = {
    name: formName.value,
    project: formProject.value,
    branch: formBranch.value,
  };
  try {
    await client.post<Sandbox>('/api/sandboxes', body);
    formName.value = '';
    formProject.value = '';
    formBranch.value = '';
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
      <div class="card" v-if="fetchedSandboxes.length === 0">Aucune sandbox.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-project">Projet</th>
            <th class="th-branch">Branche</th>
            <th class="th-phase">Phase</th>
            <th class="th-toolchains">Toolchains</th>
            <th class="th-action"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in fetchedSandboxes" :key="s.metadata.name">
            <td>{{ s.metadata.name }}</td>
            <td>{{ s.spec.project }}</td>
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
              <button class="btn btn-open" @click="openSandbox(s.metadata.name)">Ouvrir</button>
              <button
                class="btn btn-suspend"
                :class="{ 'btn-suspended': s.spec.suspended }"
                @click="suspendSandbox(s.metadata.name)"
              >
                {{ s.spec.suspended ? 'Reprendre' : 'Suspendre' }}
              </button>
              <button class="btn btn-delete" @click="deleteSandbox(s.metadata.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="card form-card">
        <h3 class="form-title">Créer une sandbox</h3>
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
          <span class="field-label">Projet</span>
          <input
            class="field-input"
            v-model="formProject"
            type="text"
            placeholder="mon-projet"
            aria-label="Projet"
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
        <button class="btn btn-create" @click="createSandbox">Créer</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.card {
  display: grid;
  grid-template-columns: 1fr 1fr;
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

.th-name,
.th-project,
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
.th-name { width: 18%; }
.th-project { width: 22%; }
.th-branch { width: 12%; }
.th-phase { width: 12%; }
.th-toolchains { width: 22%; }
.th-action { text-align: right; width: 12%; }

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

.btn-create {
  background: #4c90f0;
  color: white;
  font-weight: 600;
  padding: 6px 16px;
}

.btn-create:hover {
  background: #3a7de0;
}

.form-card {
  grid-template-columns: 1fr;
  padding: 24px 28px;
}

.form-title {
  grid-column: 1 / -1;
  margin: 0 0 12px 0;
  font-size: 15px;
  font-weight: 600;
  color: #e6e9f0;
}
</style>