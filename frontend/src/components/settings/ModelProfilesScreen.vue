<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { DialogClose } from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';
import { useCrudResource } from '../../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';

interface ModelProfile {
  name: string;
  provider: string;
  model: string;
  temperature?: number | null;
  max_tokens?: number | null;
}

interface CreateModelProfile {
  name: string;
  provider: string;
  model: string;
  temperature?: number;
  max_tokens?: number;
}

interface UpdateModelProfile {
  provider?: string;
  model?: string;
  temperature?: number;
  max_tokens?: number;
}

interface LlmProvider {
  name: string;
  provider_type: string;
  endpoint: string;
  available_models: string[];
  is_default: boolean;
}

const client = createApiClient();
const resource = useCrudResource<ModelProfile>(client, '/api/model-profiles');
const { items: fetchedProfiles, loading, error } = resource;

const providers = ref<LlmProvider[]>([]);
const providersError = ref<string | null>(null);

// Formulaire de création
const formName = ref('');
const formProvider = ref('');
const formModel = ref('');
const formTemperature = ref('');
const formMaxTokens = ref('');
const formAvailableModels = ref<string[]>([]);
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingName = ref<string | null>(null);
const editProvider = ref('');
const editModel = ref('');
const editAvailableModels = ref<string[]>([]);
const editTemperature = ref('');
const editMaxTokens = ref('');
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchProviders() {
  try {
    providers.value = await client.get<LlmProvider[]>('/api/llm-providers');
  } catch (e) {
    // Erreur de chargement des providers consignée (visuelle via providersError)
    // mais ne bloque pas l'affichage des profils existants.
    providersError.value = e instanceof ApiError ? e.message : String(e);
  }
}

/** Lorsque les providers sont chargés (après startEdit), pré-remplir les modèles. */
watch(() => providers.value, () => {
  if (editProvider.value) {
    editAvailableModels.value = modelsForProvider(editProvider.value);
  }
}, { immediate: false });

onMounted(async () => {
  await resource.fetch();
  await fetchProviders();
});

/** available_models du provider donné (dépendance du select modèle). */
function modelsForProvider(name: string): string[] {
  return providers.value.find((p) => p.name === name)?.available_models ?? [];
}

function onFormProviderChange() {
  formAvailableModels.value = modelsForProvider(formProvider.value);
  formModel.value = '';
}

function onEditProviderChange() {
  editAvailableModels.value = modelsForProvider(editProvider.value);
  editModel.value = '';
}

async function createProfile() {
  creationError.value = null;
  const body: CreateModelProfile = {
    name: formName.value,
    provider: formProvider.value,
    model: formModel.value,
    ...(formTemperature.value ? { temperature: Number(formTemperature.value) } : {}),
    ...(formMaxTokens.value ? { max_tokens: Number(formMaxTokens.value) } : {}),
  };
  try {
    await resource.create(body);
    formName.value = '';
    formProvider.value = '';
    formModel.value = '';
    formTemperature.value = '';
    formMaxTokens.value = '';
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function startEdit(profile: ModelProfile) {
  editingName.value = profile.name;
  editProvider.value = profile.provider;
  editAvailableModels.value = modelsForProvider(profile.provider);
  editModel.value = profile.model;
  editTemperature.value = profile.temperature?.toString() ?? '';
  editMaxTokens.value = profile.max_tokens?.toString() ?? '';
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editProvider.value = '';
  editAvailableModels.value = [];
  editModel.value = '';
  editTemperature.value = '';
  editMaxTokens.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(name: string) {
  editError.value = null;
  const body: UpdateModelProfile = {
    ...(editProvider.value ? { provider: editProvider.value } : {}),
    ...(editModel.value ? { model: editModel.value } : {}),
    ...(editTemperature.value ? { temperature: Number(editTemperature.value) } : {}),
    ...(editMaxTokens.value ? { max_tokens: Number(editMaxTokens.value) } : {}),
  };
  try {
    await resource.update(name, body);
    cancelEdit();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProfile(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedProfiles.length === 0" message="Aucun profil de modèle." />
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-provider">Provid.</th>
            <th class="th-model">Modèle</th>
            <th class="th-temp">Temp.</th>
            <th class="th-tokens">Max tokens</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in fetchedProfiles" :key="p.name">
            <td>{{ p.name }}</td>
            <td class="th-provider">{{ p.provider }}</td>
            <td class="th-model">{{ p.model }}</td>
            <td class="th-temp">{{ p.temperature ?? '—' }}</td>
            <td class="th-tokens">{{ p.max_tokens ?? '—' }}</td>
            <td class="th-actions">
              <button class="btn btn-edit" @click="startEdit(p)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteProfile(p.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un profil</button>

      <ErrorCard v-if="providersError" :message="providersError" />

      <DialogShell v-model:open="createModalOpen" title="Créer un profil">
        <Field label="Nom">
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="chat-moderate"
            aria-label="Nom du profil"
          />
        </Field>
        <Field label="Provider">
          <select
            class="field-input"
            v-model="formProvider"
            aria-label="Provider"
            @change="onFormProviderChange"
          >
            <option value="">—</option>
            <option v-for="p in providers" :key="p.name" :value="p.name">
              {{ p.name }}
            </option>
          </select>
        </Field>
        <Field label="Modèle">
          <select
            class="field-input"
            v-model="formModel"
            aria-label="Modèle"
          >
            <option value="">—</option>
            <option v-for="m in formAvailableModels" :key="m" :value="m">
              {{ m }}
            </option>
          </select>
          <p v-if="formProvider && formAvailableModels.length === 0" class="empty-state">
            Aucun modèle disponible — lancez un test sur ce provider.
          </p>
        </Field>
        <Field label="Température (optionnel)">
          <input
            class="field-input"
            v-model="formTemperature"
            type="number"
            step="0.1"
            min="0"
            max="2"
            placeholder="0.7"
            aria-label="Température"
          />
        </Field>
        <Field label="Max tokens (optionnel)">
          <input
            class="field-input"
            v-model="formMaxTokens"
            type="number"
            placeholder="4096"
            aria-label="Max tokens"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createProfile">Créer</button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>

      <DialogShell v-model:open="editModalOpen" :title="`Modifier : ${editingName}`">
        <Field label="Provider">
          <select
            class="field-input"
            v-model="editProvider"
            aria-label="Provider"
            @change="onEditProviderChange"
          >
            <option value="">—</option>
            <option v-for="p in providers" :key="p.name" :value="p.name">
              {{ p.name }}
            </option>
          </select>
        </Field>
        <Field label="Modèle">
          <select
            class="field-input"
            v-model="editModel"
            aria-label="Modèle"
          >
            <option value="">—</option>
            <option v-for="m in editAvailableModels" :key="m" :value="m">
              {{ m }}
            </option>
          </select>
          <p v-if="editProvider && editAvailableModels.length === 0" class="empty-state">
            Aucun modèle disponible — lancez un test sur ce provider.
          </p>
        </Field>
        <Field label="Température">
          <input
            class="field-input"
            v-model="editTemperature"
            type="number"
            step="0.1"
            min="0"
            max="2"
            aria-label="Température"
          />
        </Field>
        <Field label="Max tokens">
          <input
            class="field-input"
            v-model="editMaxTokens"
            type="number"
            aria-label="Max tokens"
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
.th-provider,
.th-model,
.th-temp,
.th-tokens,
.th-actions {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}
.th-name { width: 18%; }
.th-provider { width: 14%; }
.th-model { width: 28%; }
.th-temp { width: 10%; }
.th-tokens { width: 12%; }
.th-actions { text-align: right; width: 18%; }

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

.empty-state {
  color: #6a7185;
  font-size: 12px;
  margin: 4px 0 0 0;
}
</style>
