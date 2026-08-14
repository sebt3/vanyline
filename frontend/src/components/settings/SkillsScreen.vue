<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';

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
const resource = useCrudResource<SkillMeta>(client, '/api/skills');
const { items: fetchedSkills, loading, error } = resource;

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

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

onMounted(resource.fetch);

async function createSkill() {
  creationError.value = null;
  const body: CreateSkill = {
    name: formName.value,
    ...(formDescription.value ? { description: formDescription.value } : {}),
    ...(formBody.value ? { body: formBody.value } : {}),
  };
  try {
    await resource.create(body);
    formName.value = '';
    formDescription.value = '';
    formBody.value = '';
    createModalOpen.value = false;
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
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editDescription.value = '';
  editBody.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(name: string) {
  editError.value = null;
  const body: UpdateSkill = {};
  if (editDescription.value) body.description = editDescription.value;
  if (editBody.value) body.body = editBody.value;
  try {
    await resource.update(name, body);
    cancelEdit();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteSkill(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedSkills.length === 0" message="Aucun skill." />
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

      <button class="btn btn-create" @click="createModalOpen = true">Créer un skill</button>

      <DialogShell v-model:open="createModalOpen" title="Créer un skill">
        <Field label="Nom" top-align>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="git-skill"
            aria-label="Nom du skill"
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
        <Field label="Body" top-align>
          <textarea
            class="field-input"
            v-model="formBody"
            rows="4"
            placeholder="Body du skill (optionnel)"
            aria-label="Body"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createSkill">Créer</button>
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
        <Field label="Body" top-align>
          <textarea
            class="field-input"
            v-model="editBody"
            rows="4"
            placeholder="Body du skill"
            aria-label="Body"
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
  margin-left: 6px;
}
.btn:first-child {
  margin-left: 0;
}
</style>
