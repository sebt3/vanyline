<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';

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
const router = useRouter();
const resource = useCrudResource<Project>(client, '/api/projects');
const { items: fetchedProjects, loading, error } = resource;

function openProject(name: string) {
  router.push(`/p/${name}`);
}

const formName = ref('');
const formRepo = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

const modalOpen = ref(false);

onMounted(resource.fetch);

async function createProject() {
  creationError.value = null;
  const body: CreateProjectBody = {
    name: formName.value,
    repoUrl: formRepo.value,
    defaultBranch: formBranch.value || undefined,
  };
  try {
    await resource.create(body);
    formName.value = '';
    formRepo.value = '';
    formBranch.value = '';
    modalOpen.value = false;
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProject(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <div class="dashboard">
    <h1>Projets</h1>
    <div class="actions-row">
      <button class="btn btn-create" @click="modalOpen = true">Créer un projet</button>
      <button class="btn btn-settings" @click="router.push('/settings')">Paramètres</button>
    </div>

    <LoadingSkeleton v-if="loading" />
    <div v-else>
      <ErrorCard v-if="error" :message="error" />
      <div v-else>
        <EmptyState v-if="fetchedProjects.length === 0" message="Aucun projet." />
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
            <tr v-for="p in fetchedProjects" :key="p.metadata.name"
                @click="openProject(p.metadata.name)" class="row-clickable">
              <td>{{ p.metadata.name }}</td>
              <td>{{ p.spec.repoUrl }}</td>
              <td>{{ p.spec.defaultBranch ?? '—' }}</td>
              <td>
                <button class="btn btn-delete" @click.stop="deleteProject(p.metadata.name)">
                  Supprimer
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <DialogShell v-model:open="modalOpen" title="Créer un projet">
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
        <template #actions>
          <button class="btn btn-create" @click="createProject">Créer</button>
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
</style>
