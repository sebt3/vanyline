<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ApiError, createApiClient } from '../../api/client';

interface ProjectSpec {
  owner: string;
  repoUrl: string;
  defaultBranch?: string | null;
}

interface Project {
  metadata: { name: string };
  spec: ProjectSpec;
}

interface CreateProjectBody {
  name: string;
  repoUrl: string;
  defaultBranch?: string;
}

const client = createApiClient();
const fetchedProjects = ref<Project[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const formName = ref('');
const formRepo = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

async function fetchProjects() {
  try {
    fetchedProjects.value = await client.get<Project[]>('/api/projects');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(fetchProjects);

async function createProject() {
  creationError.value = null;
  const body: CreateProjectBody = {
    name: formName.value,
    repoUrl: formRepo.value,
    defaultBranch: formBranch.value || undefined,
  };
  try {
    await client.post<Project>('/api/projects', body);
    formName.value = '';
    formRepo.value = '';
    formBranch.value = '';
    await fetchProjects();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProject(name: string) {
  try {
    await client.delete(`/api/projects/${name}`);
    await fetchProjects();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
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
      <div class="card" v-if="fetchedProjects.length === 0">Aucun projet.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-repo">Repo</th>
            <th class="th-branch">Branche par défaut</th>
            <th class="th-action"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in fetchedProjects" :key="p.metadata.name">
            <td>{{ p.metadata.name }}</td>
            <td>{{ p.spec.repoUrl }}</td>
            <td>{{ p.spec.defaultBranch ?? '—' }}</td>
            <td>
              <button class="btn btn-delete" @click="deleteProject(p.metadata.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="card form-card">
        <h3 class="form-title">Créer un projet</h3>
        <label class="field">
          <span class="field-label">Nom</span>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-projet"
            aria-label="Nom du projet"
          />
        </label>
        <label class="field">
          <span class="field-label">Repo URL</span>
          <input
            class="field-input"
            v-model="formRepo"
            type="text"
            placeholder="https://github.com/org/repo"
            aria-label="URL du dépôt"
          />
        </label>
        <label class="field">
          <span class="field-label">Branche par défaut</span>
          <input
            class="field-input"
            v-model="formBranch"
            type="text"
            placeholder="main (optionnel)"
            aria-label="Branche par défaut"
          />
        </label>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <button class="btn btn-create" @click="createProject">Créer</button>
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
.th-repo,
.th-branch,
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
.th-repo { width: 45%; }
.th-branch { width: 20%; }
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

.btn-delete {
  background: #5b1e3f22;
  color: #e85d5d;
  border: 1px solid #e85d5d44;
}

.btn-delete:hover {
  background: #e85d5d33;
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