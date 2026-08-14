<script setup lang="ts">
import { onMounted, ref } from 'vue';
import {
  DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose,
} from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';

type AgentMode = 'primary' | 'subagent' | 'all';
type SkillSelection = 'auto' | 'none' | string[];

interface Agent {
  name: string;
  description?: string | null;
  mode: AgentMode;
  model: string;
  toolsets: string[];
  skills: SkillSelection;
  system_prompt: string;
}

interface CreateAgent {
  name: string;
  description?: string;
  mode?: AgentMode;
  model: string;
  toolsets?: string[];
  skills?: SkillSelection;
  system_prompt?: string;
}

interface UpdateAgent {
  description?: string;
  mode?: AgentMode;
  model?: string;
  toolsets?: string[];
  skills?: SkillSelection;
  system_prompt?: string;
}

interface ModelProfileOption {
  name: string;
}

interface ToolsetOption {
  name: string;
}

interface SkillOption {
  name: string;
  description: string;
}

const client = createApiClient();
const fetchedAgents = ref<Agent[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const modelProfiles = ref<ModelProfileOption[]>([]);
const toolsetOptions = ref<ToolsetOption[]>([]);
const skillOptions = ref<SkillOption[]>([]);
const optionsError = ref<string | null>(null);

// Formulaire de création
const formName = ref('');
const formDescription = ref('');
const formMode = ref<AgentMode>('primary');
const formModel = ref('');
const formToolsets = ref<string[]>([]);
const formSkills = ref<'auto' | 'none'>('auto');
const formSystemPrompt = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingName = ref<string | null>(null);
const editDescription = ref('');
const editMode = ref<AgentMode>('primary');
const editModel = ref('');
const editToolsets = ref<string[]>([]);
const editSkills = ref<'auto' | 'none'>('auto');
const editSkillList = ref<string[]>([]);
const editingSkillsIsList = ref(false);
const editSystemPrompt = ref('');
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchAgents() {
  try {
    fetchedAgents.value = await client.get<Agent[]>('/api/agents');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

async function fetchOptions() {
  try {
    const [mp, ts, sk] = await Promise.all([
      client.get<ModelProfileOption[]>('/api/model-profiles'),
      client.get<ToolsetOption[]>('/api/toolsets'),
      client.get<SkillOption[]>('/api/skills'),
    ]);
    modelProfiles.value = mp;
    toolsetOptions.value = ts;
    skillOptions.value = sk;
  } catch (e) {
    optionsError.value = e instanceof ApiError ? e.message : String(e);
    loading.value = false;
  }
}

onMounted(async () => {
  await Promise.all([fetchAgents(), fetchOptions()]);
});

function skillsToDisplay(s: SkillSelection): string {
  if (s === 'auto' || s === 'none') return s;
  if (Array.isArray(s)) return s.length ? s.join(', ') : '—';
  return '—';
}

async function createAgent() {
  creationError.value = null;
  const body: CreateAgent = {
    name: formName.value,
    ...(formDescription.value ? { description: formDescription.value } : {}),
    mode: formMode.value,
    model: formModel.value,
    toolsets: formToolsets.value.length ? formToolsets.value : undefined,
    skills: formSkills.value,
    ...(formSystemPrompt.value ? { system_prompt: formSystemPrompt.value } : {}),
  };
  try {
    await client.post<Agent>('/api/agents', body);
    formName.value = '';
    formDescription.value = '';
    formMode.value = 'primary';
    formModel.value = '';
    formToolsets.value = [];
    formSkills.value = 'auto';
    formSystemPrompt.value = '';
    createModalOpen.value = false;
    await fetchAgents();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function startEdit(agent: Agent) {
  editingName.value = agent.name;
  editDescription.value = agent.description ?? '';
  editMode.value = agent.mode;
  editModel.value = agent.model;
  editToolsets.value = [...agent.toolsets];
  if (Array.isArray(agent.skills)) {
    editingSkillsIsList.value = true;
    editSkillList.value = [...agent.skills];
  } else {
    editingSkillsIsList.value = false;
    editSkills.value = agent.skills as 'auto' | 'none';
  }
  editSystemPrompt.value = agent.system_prompt ?? '';
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editDescription.value = '';
  editMode.value = 'primary';
  editModel.value = '';
  editToolsets.value = [];
  editSkillList.value = [];
  editingSkillsIsList.value = false;
  editSkills.value = 'auto';
  editSystemPrompt.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(name: string) {
  editError.value = null;
  const body: UpdateAgent = {};
  if (editDescription.value) body.description = editDescription.value;
  body.mode = editMode.value;
  if (editModel.value) body.model = editModel.value;
  if (editToolsets.value.length > 0) body.toolsets = editToolsets.value;
  body.skills = editingSkillsIsList.value ? editSkillList.value : editSkills.value;
  if (editSystemPrompt.value) body.system_prompt = editSystemPrompt.value;
  try {
    await client.put<Agent>(`/api/agents/${name}`, body);
    cancelEdit();
    await fetchAgents();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteAgent(name: string) {
  try {
    await client.delete(`/api/agents/${name}`);
    await fetchAgents();
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
      <div class="card" v-if="fetchedAgents.length === 0">Aucun agent.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-desc">Description</th>
            <th class="th-mode">Mode</th>
            <th class="th-model">Modèle</th>
            <th class="th-toolsets">Toolsets</th>
            <th class="th-skills">Skills</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="a in fetchedAgents" :key="a.name">
            <td>{{ a.name }}</td>
            <td>{{ a.description ?? '—' }}</td>
            <td>{{ a.mode }}</td>
            <td>{{ a.model }}</td>
            <td>
              {{ a.toolsets.length ? a.toolsets.join(', ') : '—' }}
            </td>
            <td>{{ skillsToDisplay(a.skills) }}</td>
            <td class="th-actions">
              <button class="btn btn-edit" @click="startEdit(a)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteAgent(a.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un agent</button>

      <div class="card" v-if="optionsError" role="alert">
        <p class="error-text">{{ optionsError }}</p>
      </div>

      <DialogRoot v-model:open="createModalOpen">
        <DialogPortal>
          <DialogOverlay class="dialog-overlay" />
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Créer un agent</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="formName"
                type="text"
                placeholder="mon-agent"
                aria-label="Nom de l'agent"
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
              <span class="field-label">Mode</span>
              <select
                class="field-input"
                v-model="formMode"
                aria-label="Mode"
              >
                <option value="primary">primary</option>
                <option value="subagent">subagent</option>
                <option value="all">all</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Profil de modèle</span>
              <select class="field-input" v-model="formModel" aria-label="Profil de modèle">
                <option value="">—</option>
                <option v-for="p in modelProfiles" :key="p.name" :value="p.name">{{ p.name }}</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Toolsets</span>
              <div class="checkbox-list">
                <label v-for="t in toolsetOptions" :key="t.name" class="checkbox-item">
                  <input type="checkbox" :value="t.name" v-model="formToolsets" />
                  <span>{{ t.name }}</span>
                </label>
              </div>
            </label>
            <label class="field">
              <span class="field-label">Skills</span>
              <select
                class="field-input"
                v-model="formSkills"
                aria-label="Skills"
              >
                <option value="auto">auto</option>
                <option value="none">none</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">System prompt</span>
              <textarea
                class="field-input"
                v-model="formSystemPrompt"
                rows="4"
                placeholder="Prompt système optionnel"
                aria-label="System prompt"
              />
            </label>
            <div v-if="creationError" class="creation-error">{{ creationError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-create" @click="createAgent">Créer</button>
              <DialogClose class="btn btn-cancel">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>

      <DialogRoot v-model:open="editModalOpen">
        <DialogPortal>
          <DialogOverlay class="dialog-overlay" />
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Modifier : {{ editingName }}</DialogTitle>
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
              <span class="field-label">Mode</span>
              <select
                class="field-input"
                v-model="editMode"
                aria-label="Mode"
              >
                <option value="primary">primary</option>
                <option value="subagent">subagent</option>
                <option value="all">all</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Profil de modèle</span>
              <select class="field-input" v-model="editModel" aria-label="Profil de modèle">
                <option value="">—</option>
                <option v-for="p in modelProfiles" :key="p.name" :value="p.name">{{ p.name }}</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Toolsets</span>
              <div class="checkbox-list">
                <label v-for="t in toolsetOptions" :key="t.name" class="checkbox-item">
                  <input type="checkbox" :value="t.name" v-model="editToolsets" />
                  <span>{{ t.name }}</span>
                </label>
              </div>
            </label>
            <template v-if="!editingSkillsIsList">
              <label class="field">
                <span class="field-label">Skills</span>
                <select
                  class="field-input"
                  v-model="editSkills"
                  aria-label="Skills"
                >
                  <option value="auto">auto</option>
                  <option value="none">none</option>
                </select>
              </label>
            </template>
            <template v-else>
              <label class="field">
                <span class="field-label">Skills</span>
                <div class="checkbox-list">
                  <label v-for="s in skillOptions" :key="s.name" class="checkbox-item">
                    <input type="checkbox" :value="s.name" v-model="editSkillList" />
                    <span>{{ s.name }}</span>
                  </label>
                </div>
              </label>
            </template>
            <label class="field">
              <span class="field-label">System prompt</span>
              <textarea
                class="field-input"
                v-model="editSystemPrompt"
                rows="4"
                placeholder="Prompt système"
                aria-label="System prompt"
              />
            </label>
            <div v-if="editError" class="creation-error">{{ editError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-success" @click="saveEdit(editingName!)">Sauvegarder</button>
              <DialogClose class="btn btn-cancel" @click="cancelEdit">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>
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
.th-mode,
.th-model,
.th-toolsets,
.th-skills,
.th-actions {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}
.th-name { width: 14%; }
.th-desc { width: 18%; }
.th-mode { width: 10%; }
.th-model { width: 18%; }
.th-toolsets { width: 14%; }
.th-skills { width: 12%; }
.th-actions { text-align: right; width: 14%; }

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

.dialog-actions {
  display: flex;
  gap: 12px;
  justify-content: flex-end;
  margin-top: 12px;
}

.dialog-actions .btn:first-child {
  margin-left: 0;
}

.checkbox-list {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.checkbox-item {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 13px;
  color: #e6e9f0;
}

.checkbox-item input[type="checkbox"] {
  width: 14px;
  height: 14px;
}
</style>

<style>
.dialog-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 1000;
}

[role='dialog'] {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  z-index: 1001;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  padding: 24px 28px;
  max-width: 480px;
  max-height: 85vh;
  overflow-y: auto;
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
