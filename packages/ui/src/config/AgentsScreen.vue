<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import type { Agent, AgentMode, ModelProfile, SkillSelection } from '../ports';
import { useConfigRepo } from './useConfigRepo';
import { useCrudResource } from '../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import CheckboxList from '../common/CheckboxList.vue';
import Field from '../common/Field.vue';
import SourceBadge from '../common/SourceBadge.vue';

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const repo = useConfigRepo();
const resource = useCrudResource(repo, 'agents');
const { items: fetchedAgents, loading, error } = resource;

const modelProfiles = ref<ModelProfile[]>([]);
const toolsetOptions = ref<string[]>([]);
const skillOptions = ref<string[]>([]);
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
      repo.list('profiles'),
      repo.list('toolsets'),
      repo.list('skills'),
    ]);
    modelProfiles.value = mp;
    toolsetOptions.value = ts.map((t) => t.name);
    skillOptions.value = sk.map((s) => s.name);
  } catch (e) {
    optionsError.value = message(e);
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
  try {
    await resource.create({
      name: formName.value,
      ...(formDescription.value ? { description: formDescription.value } : {}),
      mode: formMode.value,
      model: formModel.value,
      toolsets: formToolsets.value,
      skills: formSkills.value,
      system_prompt: formSystemPrompt.value,
    });
    formName.value = '';
    formDescription.value = '';
    formMode.value = 'primary';
    formModel.value = '';
    formToolsets.value = [];
    formSkills.value = 'auto';
    formSystemPrompt.value = '';
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = message(e);
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
    editSkills.value = agent.skills;
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
  const patch: Partial<Agent> = {};
  if (editDescription.value) patch.description = editDescription.value;
  patch.mode = editMode.value;
  if (editModel.value) patch.model = editModel.value;
  if (editToolsets.value.length > 0) patch.toolsets = editToolsets.value;
  patch.skills = editingSkillsIsList.value ? editSkillList.value : editSkills.value;
  if (editSystemPrompt.value) patch.system_prompt = editSystemPrompt.value;
  try {
    await resource.update(name, patch);
    cancelEdit();
  } catch (e) {
    editError.value = message(e);
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
            <td>{{ a.name }} <SourceBadge :source="a.source" /></td>
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
        <Field label="Nom" top-align>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-agent"
            aria-label="Nom de l'agent"
          />
        </Field>
        <Field label="Description" top-align>
          <textarea
            class="field-input"
            v-model="formDescription"
            rows="2"
            placeholder="Description optionnelle"
            aria-label="Description"
          />
        </Field>
        <Field label="Mode" top-align>
          <select
            class="field-input"
            v-model="formMode"
            aria-label="Mode"
          >
            <option value="primary">primary</option>
            <option value="subagent">subagent</option>
            <option value="all">all</option>
          </select>
        </Field>
        <Field label="Profil de modèle" top-align>
          <select class="field-input" v-model="formModel" aria-label="Profil de modèle">
            <option value="">—</option>
            <option v-for="p in modelProfiles" :key="p.name" :value="p.name">{{ p.name }}</option>
          </select>
        </Field>
        <Field label="Toolsets" top-align>
          <CheckboxList :options="toOptions(toolsetOptions)" v-model="formToolsets" />
        </Field>
        <Field label="Skills" top-align>
          <select
            class="field-input"
            v-model="formSkills"
            aria-label="Skills"
          >
            <option value="auto">auto</option>
            <option value="none">none</option>
          </select>
        </Field>
        <Field label="System prompt" top-align>
          <textarea
            class="field-input"
            v-model="formSystemPrompt"
            rows="4"
            placeholder="Prompt système optionnel"
            aria-label="System prompt"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createAgent">Créer</button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>

      <DialogShell v-model:open="editModalOpen" :title="`Modifier : ${editingName}`">
        <Field label="Description" top-align>
          <textarea
            class="field-input"
            v-model="editDescription"
            rows="2"
            placeholder="Description"
            aria-label="Description"
          />
        </Field>
        <Field label="Mode" top-align>
          <select
            class="field-input"
            v-model="editMode"
            aria-label="Mode"
          >
            <option value="primary">primary</option>
            <option value="subagent">subagent</option>
            <option value="all">all</option>
          </select>
        </Field>
        <Field label="Profil de modèle" top-align>
          <select class="field-input" v-model="editModel" aria-label="Profil de modèle">
            <option value="">—</option>
            <option v-for="p in modelProfiles" :key="p.name" :value="p.name">{{ p.name }}</option>
          </select>
        </Field>
        <Field label="Toolsets" top-align>
          <CheckboxList :options="toOptions(toolsetOptions)" v-model="editToolsets" />
        </Field>
        <Field v-if="!editingSkillsIsList" label="Skills" top-align>
          <select
            class="field-input"
            v-model="editSkills"
            aria-label="Skills"
          >
            <option value="auto">auto</option>
            <option value="none">none</option>
          </select>
        </Field>
        <Field v-else label="Skills" top-align>
          <CheckboxList :options="toOptions(skillOptions)" v-model="editSkillList" />
        </Field>
        <Field label="System prompt" top-align>
          <textarea
            class="field-input"
            v-model="editSystemPrompt"
            rows="4"
            placeholder="Prompt système"
            aria-label="System prompt"
          />
        </Field>
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
</style>
