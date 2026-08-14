<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import {
  DialogRoot, DialogPortal, DialogContent, DialogTitle, DialogClose,
} from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';

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
const fetchedProfiles = ref<ModelProfile[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

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

async function fetchProfiles() {
  try {
    fetchedProfiles.value = await client.get<ModelProfile[]>('/api/model-profiles');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

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
  // Fetch des profils (gère loading/error)
  await fetchProfiles();
  // Fetch des providers (erreurs non-critiques : affichées sans bloquer)
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
    await client.post<ModelProfile>('/api/model-profiles', body);
    formName.value = '';
    formProvider.value = '';
    formModel.value = '';
    formTemperature.value = '';
    formMaxTokens.value = '';
    createModalOpen.value = false;
    await fetchProfiles();
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
    await client.put<ModelProfile>(`/api/model-profiles/${name}`, body);
    cancelEdit();
    await fetchProfiles();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProfile(name: string) {
  try {
    await client.delete(`/api/model-profiles/${name}`);
    await fetchProfiles();
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
      <div class="card" v-if="fetchedProfiles.length === 0">Aucun profil de modèle.</div>
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

      <div class="card" v-if="providersError" role="alert">
        <p class="error-text">{{ providersError }}</p>
      </div>

      <DialogRoot v-model:open="createModalOpen">
        <DialogPortal>
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Créer un profil</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="formName"
                type="text"
                placeholder="chat-moderate"
                aria-label="Nom du profil"
              />
            </label>
            <label class="field">
              <span class="field-label">Provider</span>
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
            </label>
            <label class="field">
              <span class="field-label">Modèle</span>
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
            </label>
            <label class="field">
              <span class="field-label">Température (optionnel)</span>
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
            </label>
            <label class="field">
              <span class="field-label">Max tokens (optionnel)</span>
              <input
                class="field-input"
                v-model="formMaxTokens"
                type="number"
                placeholder="4096"
                aria-label="Max tokens"
              />
            </label>
            <div v-if="creationError" class="creation-error">{{ creationError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-create" @click="createProfile">Créer</button>
              <DialogClose class="btn btn-cancel">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>

      <DialogRoot v-model:open="editModalOpen">
        <DialogPortal>
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Modifier : {{ editingName }}</DialogTitle>
            <label class="field">
              <span class="field-label">Provider</span>
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
            </label>
            <label class="field">
              <span class="field-label">Modèle</span>
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
            </label>
            <label class="field">
              <span class="field-label">Température</span>
              <input
                class="field-input"
                v-model="editTemperature"
                type="number"
                step="0.1"
                min="0"
                max="2"
                aria-label="Température"
              />
            </label>
            <label class="field">
              <span class="field-label">Max tokens</span>
              <input
                class="field-input"
                v-model="editMaxTokens"
                type="number"
                aria-label="Max tokens"
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

.empty-state {
  color: #6a7185;
  font-size: 12px;
  margin: 4px 0 0 0;
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
