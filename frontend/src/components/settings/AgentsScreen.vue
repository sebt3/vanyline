<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import CheckboxList from '../common/CheckboxList.vue';

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
const resource = useCrudResource<Agent>(client, '/api/agents');
const { items: fetchedAgents, loading, error } = resource;

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
  }
}

onMounted(async () => {
  await Promise.all([resource.fetch(), fetchOptions()]);
});

function skillsToDisplay(s: SkillSelection): string {
  if (s === 'auto' || s === 'none') return s;
  if (Array.isArray(s)) return s.length ? s.join(', ') : '—';
  return '—';
}

function toOptions(names: string[]): { value: string; label: string }[] {
  return names.map((name) => ({ value: name, label: name }));
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
    await resource.create(body);
    formName.value = '';
    formDescription.value = '';
    formMode.value = 'primary';
    formModel.value = '';
    formToolsets.value = [];
    formSkills.value = 'auto';
    formSystemPrompt.value = '';
    createModalOpen.value = false;
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
    await resource.update(name, body);
    cancelEdit();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteAgent(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedAgents.length === 0" message="Aucun agent." />
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

      <ErrorCard v-if="optionsError" :message="optionsError" />

      <DialogShell v-model:open="createModalOpen" title="Créer un agent">
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
          <CheckboxList :options="toOptions(toolsetOptions.map((t) => t.name))" v-model="formToolsets" />
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
        <template #actions>
          <button class="btn btn-create" @click="createAgent">Créer</button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>

      <DialogShell v-model:open="editModalOpen" :title="`Modifier : ${editingName}`">
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
          <CheckboxList :options="toOptions(toolsetOptions.map((t) => t.name))" v-model="editToolsets" />
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
            <CheckboxList :options="toOptions(skillOptions.map((s) => s.name))" v-model="editSkillList" />
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
        <template #actions>
          <button class="btn btn-success" @click="saveEdit(editingName!)">Sauvegarder</button>
          <DialogClose class="btn btn-cancel" @click="cancelEdit">Annuler</DialogClose>
        </template>
      </DialogShell>
    </div>
  </div>
</template>

<style scoped>
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
  margin-left: 6px;
}
.btn:first-child {
  margin-left: 0;
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
</style>
