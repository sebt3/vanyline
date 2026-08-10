<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ApiError, createApiClient } from '../../api/client';

interface SkillMeta {
  name: string;
  description: string;
}

interface SkillDetail {
  name: string;
  description: string;
  body: string;
}

interface CreateSkill {
  name: string;
  description?: string;
  body?: string;
}

interface UpdateSkill {
  description?: string;
  body?: string;
}

const client = createApiClient();
const fetchedSkills = ref<SkillMeta[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

// Formulaire de création
const formName = ref('');
const formDescription = ref('');
const formBody = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition — séparé car la liste n'expédie pas `body`
const editingName = ref<string | null>(null);
const editDescription = ref('');
const editBody = ref('');
const editError = ref<string | null>(null);

async function fetchSkills() {
  try {
    fetchedSkills.value = await client.get<SkillMeta[]>('/api/skills');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(fetchSkills);

async function createSkill() {
  creationError.value = null;
  const body: CreateSkill = {
    name: formName.value,
    ...(formDescription.value ? { description: formDescription.value } : {}),
    ...(formBody.value ? { body: formBody.value } : {}),
  };
  try {
    await client.post<SkillDetail>('/api/skills', body);
    formName.value = '';
    formDescription.value = '';
    formBody.value = '';
    await fetchSkills();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function editSkill(name: string) {
  editingName.value = name;
  editError.value = null;
  try {
    // La liste ne contient pas `body` → appel dédié pour charger le détail
    const detail = await client.get<SkillDetail>(`/api/skills/${name}`);
    editDescription.value = detail.description ?? '';
    editBody.value = detail.body ?? '';
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function cancelEdit() {
  editingName.value = null;
  editDescription.value = '';
  editBody.value = '';
  editError.value = null;
}

async function saveEdit(name: string) {
  editError.value = null;
  const body: UpdateSkill = {};
  if (editDescription.value) body.description = editDescription.value;
  if (editBody.value) body.body = editBody.value;
  try {
    await client.put<SkillDetail>(`/api/skills/${name}`, body);
    cancelEdit();
    await fetchSkills();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteSkill(name: string) {
  try {
    await client.delete(`/api/skills/${name}`);
    await fetchSkills();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}
</script>

<template>
  <div v-if="loading" class="skeleton-card">
    <div class="skeleton" />
    <div class="skeleton short" />
    <div class="skeleton short" />
  </div>
  <div v-else>
    <div class="card" v-if="error" role="alert">
      <p class="error-text">{{ error }}</p>
    </div>
    <div v-else>
      <div class="card" v-if="fetchedSkills.length === 0">Aucun skill.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-desc">Description</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in fetchedSkills" :key="s.name">
            <td>{{ s.name }}</td>
            <td>{{ s.description ?? '—' }}</td>
            <td class="th-actions">
              <button class="btn btn-edit" @click="editSkill(s.name)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteSkill(s.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="card form-card">
        <h3 class="form-title">Créer un skill</h3>
        <label class="field">
          <span class="field-label">Nom</span>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="git-skill"
            aria-label="Nom du skill"
          />
        </label>
        <label class="field">
          <span class="field-label">Description</span>
          <textarea
            class="field-input"
            v-model="formDescription"
            rows="2"
            placeholder="Description optionnelle"
            aria-label="Description"
          />
        </label>
        <label class="field">
          <span class="field-label">Body</span>
          <textarea
            class="field-input"
            v-model="formBody"
            rows="4"
            placeholder="Body du skill (optionnel)"
            aria-label="Body"
          />
        </label>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <button class="btn btn-create" @click="createSkill">Créer</button>
      </div>

      <template v-for="s in fetchedSkills" :key="'edit-' + s.name">
        <div v-if="s.name === editingName" class="card form-card">
          <h3 class="form-title">Modifier : {{ s.name }}</h3>
          <label class="field">
            <span class="field-label">Description</span>
            <textarea
              class="field-input"
              v-model="editDescription"
              rows="2"
              placeholder="Description"
              aria-label="Description"
            />
          </label>
          <label class="field">
            <span class="field-label">Body</span>
            <textarea
              class="field-input"
              v-model="editBody"
              rows="4"
              placeholder="Body du skill"
              aria-label="Body"
            />
          </label>
          <div v-if="editError" class="creation-error">{{ editError }}</div>
          <div class="edit-actions">
            <button class="btn btn-success" @click="saveEdit(s.name)">Sauvegarder</button>
            <button class="btn btn-cancel" @click="cancelEdit">Annuler</button>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.skeleton-card {
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
.th-desc,
.th-actions {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}
.th-name { width: 45%; }
.th-desc { width: 38%; }
.th-actions { text-align: right; width: 17%; }

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
  margin-left: 6px;
}
.btn:first-child {
  margin-left: 0;
}

.btn-edit {
  background: #3fb56d22;
  color: #3fb56d;
  border: 1px solid #3fb56d44;
}
.btn-edit:hover {
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

.btn-create {
  background: #4c90f0;
  color: white;
  font-weight: 600;
  padding: 6px 16px;
}
.btn-create:hover {
  background: #3a7de0;
}

.btn-cancel {
  background: #1c1c2a;
  color: #9497a9;
  padding: 6px 16px;
}
.btn-cancel:hover {
  background: #26263a;
  color: white;
}

.btn-success {
  background: #3fb56d;
  color: white;
  padding: 6px 16px;
}

.field {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 8px 12px;
  align-items: start;
  margin-bottom: 12px;
}

.field-label {
  font-size: 12px;
  font-weight: 600;
  color: #6a7185;
  text-transform: uppercase;
  padding-top: 6px;
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

.edit-actions {
  display: flex;
  gap: 8px;
}
</style>