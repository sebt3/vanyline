<script setup lang="ts">
import { onMounted, ref } from 'vue';
import {
  DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose,
} from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';

interface LlmProvider {
  id: string;
  name: string;
  provider_type: string;
  endpoint: string;
  api_key?: string | null;
  available_models?: unknown;
  is_default: boolean;
}

interface CreateLlmProvider {
  name: string;
  provider_type: string;
  endpoint: string;
  api_key?: string;
}

interface UpdateLlmProvider {
  name?: string;
  provider_type?: string;
  endpoint?: string;
  api_key?: string;
}

interface TestResult {
  models: string[];
}

const client = createApiClient();
const fetchedProviders = ref<LlmProvider[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

// Formulaire de création
const formName = ref('');
const formProviderType = ref('ollama');
const formEndpoint = ref('');
const formApiKey = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingId = ref<string | null>(null);
const editName = ref('');
const editProviderType = ref('ollama');
const editEndpoint = ref('');
const editApiKey = ref('');
const editError = ref<string | null>(null);

// Résultat du test
const testResults = ref<Record<string, string>>({});

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchProviders() {
  try {
    fetchedProviders.value = await client.get<LlmProvider[]>('/api/llm-providers');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(fetchProviders);

async function createProvider() {
  creationError.value = null;
  const body: CreateLlmProvider = {
    name: formName.value,
    provider_type: formProviderType.value,
    endpoint: formEndpoint.value,
    ...(formApiKey.value ? { api_key: formApiKey.value } : {}),
  };
  try {
    await client.post<LlmProvider>('/api/llm-providers', body);
    formName.value = '';
    formProviderType.value = 'ollama';
    formEndpoint.value = '';
    formApiKey.value = '';
    createModalOpen.value = false;
    await fetchProviders();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function startEdit(provider: LlmProvider) {
  editingId.value = provider.id;
  editName.value = provider.name;
  editProviderType.value = provider.provider_type;
  editEndpoint.value = provider.endpoint;
  editApiKey.value = provider.api_key ?? '';
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingId.value = null;
  editName.value = '';
  editProviderType.value = 'ollama';
  editEndpoint.value = '';
  editApiKey.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(id: string) {
  editError.value = null;
  const body: UpdateLlmProvider = {
    name: editName.value,
    provider_type: editProviderType.value,
    endpoint: editEndpoint.value,
    ...(editApiKey.value ? { api_key: editApiKey.value } : {}),
  };
  try {
    await client.put<LlmProvider>(`/api/llm-providers/${id}`, body);
    cancelEdit();
    await fetchProviders();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function testProvider(id: string) {
  try {
    const result = await client.post<TestResult>(`/api/llm-providers/${id}/test`);
    testResults.value[id] = result.models.join(', ');
  } catch (e) {
    testResults.value[id] = e instanceof ApiError ? e.message : String(e);
  }
}

async function setDefault(id: string) {
  try {
    await client.put<LlmProvider>(`/api/llm-providers/${id}/default`);
    await fetchProviders();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteProvider(id: string) {
  try {
    await client.delete(`/api/llm-providers/${id}`);
    await fetchProviders();
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
      <div class="card" v-if="fetchedProviders.length === 0">Aucun fournisseur LLM.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-type">Type</th>
            <th class="th-endpoint">Endpoint</th>
            <th class="th-status">Statut</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in fetchedProviders" :key="p.id">
            <td>{{ p.name }}</td>
            <td class="th-type">{{ p.provider_type }}</td>
            <td class="th-endpoint">{{ p.endpoint }}</td>
            <td class="th-status">
              <span v-if="p.is_default" class="badge-default">Défaut</span>
            </td>
            <td class="th-actions">
              <button class="btn btn-test" @click="testProvider(p.id)">
                Tester
              </button>
              <button class="btn btn-default" :class="{ 'btn-default-active': p.is_default }" @click="setDefault(p.id)">
                Défaut
              </button>
              <button class="btn btn-edit" @click="startEdit(p)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteProvider(p.id)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un fournisseur</button>

      <DialogRoot v-model:open="createModalOpen">
        <DialogPortal>
          <DialogOverlay class="dialog-overlay" />
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Créer un fournisseur</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="formName"
                type="text"
                placeholder="mon-fournisseur"
                aria-label="Nom du fournisseur"
              />
            </label>
            <label class="field">
              <span class="field-label">Type</span>
              <select
                class="field-input"
                v-model="formProviderType"
                aria-label="Type de fournisseur"
              >
                <option value="ollama">ollama</option>
                <option value="openai-compatible">openai-compatible</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Endpoint</span>
              <input
                class="field-input"
                v-model="formEndpoint"
                type="text"
                placeholder="http://localhost:11434"
                aria-label="Endpoint"
              />
            </label>
            <label class="field">
              <span class="field-label">Clé API (optionnel)</span>
              <input
                class="field-input"
                v-model="formApiKey"
                type="text"
                placeholder="sk-..."
                aria-label="Clé API"
              />
            </label>
            <div v-if="creationError" class="creation-error">{{ creationError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-create" @click="createProvider">Créer</button>
              <DialogClose class="btn btn-cancel">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>

      <DialogRoot v-model:open="editModalOpen">
        <DialogPortal>
          <DialogOverlay class="dialog-overlay" />
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Modifier : {{ editName }}</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="editName"
                type="text"
                placeholder="nom"
                aria-label="Nom"
              />
            </label>
            <label class="field">
              <span class="field-label">Type</span>
              <select
                class="field-input"
                v-model="editProviderType"
                aria-label="Type de fournisseur"
              >
                <option value="ollama">ollama</option>
                <option value="openai-compatible">openai-compatible</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">Endpoint</span>
              <input
                class="field-input"
                v-model="editEndpoint"
                type="text"
                placeholder="http://localhost:11434"
                aria-label="Endpoint"
              />
            </label>
            <label class="field">
              <span class="field-label">Clé API (optionnel)</span>
              <input
                class="field-input"
                v-model="editApiKey"
                type="text"
                placeholder="sk-..."
                aria-label="Clé API"
              />
            </label>
            <div v-if="editError" class="creation-error">{{ editError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-success" @click="saveEdit(editingId!)">Sauvegarder</button>
              <DialogClose class="btn btn-cancel" @click="cancelEdit">Annuler</DialogClose>
            </div>
          </DialogContent>
        </DialogPortal>
      </DialogRoot>

      <div v-for="p in fetchedProviders" :key="'test-' + p.id" class="results">
        <div v-if="testResults[p.id]" class="test-result">
          Résultat pour {{ p.name }} : {{ testResults[p.id] }}
        </div>
      </div>
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
.th-type,
.th-endpoint,
.th-status,
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
.th-type { width: 18%; }
.th-endpoint { width: 35%; }
.th-status { width: 12%; }
.th-actions { text-align: right; width: 17%; }

.table td {
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
}

.badge-default {
  display: inline-block;
  background: #3fb56d22;
  color: #3fb56d;
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 9999px;
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

.btn-test {
  background: #4c90f022;
  color: #4c90f0;
  border: 1px solid #4c90f044;
}
.btn-test:hover {
  background: #4c90f033;
}

.btn-default {
  background: #e0a83d22;
  color: #e0a83d;
  border: 1px solid #e0a83d44;
}
.btn-default:hover {
  background: #e0a83d33;
}
.btn-default-active {
  background: #e0a83d;
  color: white;
  font-weight: 600;
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

.dialog-actions {
  display: flex;
  gap: 12px;
  justify-content: flex-end;
  margin-top: 12px;
}

.dialog-actions .btn:first-child {
  margin-left: 0;
}

.results {
  max-width: 760px;
  margin-bottom: 12px;
}

.test-result {
  padding: 12px 20px;
  background: #3fb56d1a;
  border: 1px solid #3fb56d;
  border-radius: 6px;
  color: #3fb56d;
  font-size: 13px;
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
