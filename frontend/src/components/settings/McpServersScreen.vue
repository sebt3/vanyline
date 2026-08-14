<script setup lang="ts">
import { onMounted, ref } from 'vue';
import {
  DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose,
} from 'reka-ui';
import { ApiError, createApiClient } from '../../api/client';

interface McpServer {
  id: string;
  name: string;
  server_type: string;
  url: string;
}

interface CreateMcpServer {
  name: string;
  server_type: string;
  url: string;
}

interface UpdateMcpServer {
  name?: string;
  server_type?: string;
  url?: string;
}

const client = createApiClient();
const fetchedServers = ref<McpServer[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

// Formulaire de création
const formName = ref('');
const formServerType = ref('sse');
const formUrl = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingId = ref<string | null>(null);
const editName = ref('');
const editServerType = ref('sse');
const editUrl = ref('');
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchServers() {
  try {
    fetchedServers.value = await client.get<McpServer[]>('/api/mcp-servers');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(fetchServers);

async function createServer() {
  creationError.value = null;
  const body: CreateMcpServer = {
    name: formName.value,
    server_type: formServerType.value,
    url: formUrl.value,
  };
  try {
    await client.post<McpServer>('/api/mcp-servers', body);
    formName.value = '';
    formServerType.value = 'sse';
    formUrl.value = '';
    createModalOpen.value = false;
    await fetchServers();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function startEdit(server: McpServer) {
  editingId.value = server.id;
  editName.value = server.name;
  editServerType.value = server.server_type;
  editUrl.value = server.url;
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingId.value = null;
  editName.value = '';
  editServerType.value = 'sse';
  editUrl.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(id: string) {
  editError.value = null;
  const body: UpdateMcpServer = {
    name: editName.value,
    server_type: editServerType.value,
    url: editUrl.value,
  };
  try {
    await client.put<McpServer>(`/api/mcp-servers/${id}`, body);
    cancelEdit();
    await fetchServers();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteServer(id: string) {
  try {
    await client.delete(`/api/mcp-servers/${id}`);
    await fetchServers();
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
      <div class="card" v-if="fetchedServers.length === 0">Aucun serveur MCP.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-type">Type</th>
            <th class="th-url">URL</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in fetchedServers" :key="s.id">
            <td>{{ s.name }}</td>
            <td>{{ s.server_type }}</td>
            <td>{{ s.url }}</td>
            <td class="th-actions">
              <button class="btn btn-edit" @click="startEdit(s)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteServer(s.id)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un serveur MCP</button>

      <DialogRoot v-model:open="createModalOpen">
        <DialogPortal>
          <DialogOverlay class="dialog-overlay" />
          <DialogContent class="dialog-content" role="dialog">
            <DialogTitle class="dialog-title">Créer un serveur MCP</DialogTitle>
            <label class="field">
              <span class="field-label">Nom</span>
              <input
                class="field-input"
                v-model="formName"
                type="text"
                placeholder="mon-mcp-server"
                aria-label="Nom du serveur"
              />
            </label>
            <label class="field">
              <span class="field-label">Type</span>
              <select
                class="field-input"
                v-model="formServerType"
                aria-label="Type de serveur"
              >
                <option value="sse">sse</option>
                <option value="http-streamable">http-streamable</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">URL</span>
              <input
                class="field-input"
                v-model="formUrl"
                type="text"
                placeholder="https://example.com/mcp"
                aria-label="URL"
              />
            </label>
            <div v-if="creationError" class="creation-error">{{ creationError }}</div>
            <div class="dialog-actions">
              <button class="btn btn-create" @click="createServer">Créer</button>
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
                placeholder="Nom du serveur"
                aria-label="Nom du serveur"
              />
            </label>
            <label class="field">
              <span class="field-label">Type</span>
              <select
                class="field-input"
                v-model="editServerType"
                aria-label="Type de serveur"
              >
                <option value="sse">sse</option>
                <option value="http-streamable">http-streamable</option>
              </select>
            </label>
            <label class="field">
              <span class="field-label">URL</span>
              <input
                class="field-input"
                v-model="editUrl"
                type="text"
                placeholder="https://example.com/mcp"
                aria-label="URL"
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

.th-name {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 28%;
}

.th-type {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 18%;
}

.th-url {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 40%;
}

.th-actions {
  text-align: right;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 14%;
}

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
