<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ApiError, createApiClient } from '../../api/client';

interface McpSelection {
  server: string;
  tools?: string[];
}

interface Toolset {
  name: string;
  description?: string | null;
  prompt?: string | null;
  local_tools: string[];
  mcp: McpSelection[];
}

interface CreateToolset {
  name: string;
  description?: string;
  prompt?: string;
  local_tools?: string[];
  mcp?: McpSelection[];
}

interface UpdateToolset {
  description?: string;
  prompt?: string;
  local_tools?: string[];
  mcp?: McpSelection[];
}

interface LocalTool {
  name: string;
  description: string;
}

interface McpServerOption {
  name: string;
  server_type: string;
  url: string;
  available_tools: string[];
}

const client = createApiClient();
const fetchedToolsets = ref<Toolset[]>([]);
const error = ref<string | null>(null);
const loading = ref(true);

const localTools = ref<LocalTool[]>([]);
const mcpServers = ref<McpServerOption[]>([]);
const optionsError = ref<string | null>(null);

const formName = ref('');
const formDescription = ref('');
const formPrompt = ref('');
const formLocalTools = ref<string[]>([]);
const formMcp = ref<McpSelection[]>([]);
const creationError = ref<string | null>(null);

const editingName = ref<string | null>(null);
const editDescription = ref('');
const editPrompt = ref('');
const editLocalTools = ref<string[]>([]);
const editMcp = ref<McpSelection[]>([]);
const editError = ref<string | null>(null);

async function fetchToolsets() {
  try {
    fetchedToolsets.value = await client.get<Toolset[]>('/api/toolsets');
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

async function fetchOptions() {
  try {
    const [lt, servers] = await Promise.all([
      client.get<LocalTool[]>('/api/local-tools'),
      client.get<McpServerOption[]>('/api/mcp-servers'),
    ]);
    localTools.value = lt;
    mcpServers.value = servers;
  } catch (e) {
    optionsError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function mcpToolsForServer(server: string): string[] {
  return mcpServers.value.find((s) => s.name === server)?.available_tools ?? [];
}

function addMcpRow() {
  formMcp.value.push({ server: '', tools: [] });
}

function removeMcpRow(index: number) {
  formMcp.value.splice(index, 1);
}

function onMcpServerChange(index: number) {
  formMcp.value[index].tools = [];
}

function addEditMcpRow() {
  editMcp.value.push({ server: '', tools: [] });
}

function removeEditMcpRow(index: number) {
  editMcp.value.splice(index, 1);
}

function onEditMcpServerChange(index: number) {
  editMcp.value[index].tools = [];
}

onMounted(() => {
  fetchToolsets();
  fetchOptions();
});

async function createToolset() {
  creationError.value = null;
  const body: CreateToolset = {
    name: formName.value,
    ...(formDescription.value ? { description: formDescription.value } : {}),
    ...(formPrompt.value ? { prompt: formPrompt.value } : {}),
    local_tools: formLocalTools.value,
    mcp: formMcp.value,
  };
  try {
    await client.post<Toolset>('/api/toolsets', body);
    formName.value = '';
    formDescription.value = '';
    formPrompt.value = '';
    formLocalTools.value = [];
    formMcp.value = [];
    await fetchToolsets();
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

function startEdit(toolset: Toolset) {
  editingName.value = toolset.name;
  editDescription.value = toolset.description ?? '';
  editPrompt.value = toolset.prompt ?? '';
  editLocalTools.value = [...toolset.local_tools];
  editMcp.value = toolset.mcp.map((m) => ({ server: m.server, tools: m.tools ?? [] }));
  editError.value = null;
}

function cancelEdit() {
  editingName.value = null;
  editDescription.value = '';
  editPrompt.value = '';
  editLocalTools.value = [];
  editMcp.value = [];
  editError.value = null;
}

async function saveEdit(name: string) {
  editError.value = null;
  const body: UpdateToolset = {};
  if (editDescription.value) body.description = editDescription.value;
  if (editPrompt.value) body.prompt = editPrompt.value;
  if (editLocalTools.value.length > 0) body.local_tools = editLocalTools.value;
  if (editMcp.value.length > 0) body.mcp = editMcp.value;
  try {
    await client.put<Toolset>(`/api/toolsets/${name}`, body);
    cancelEdit();
    await fetchToolsets();
  } catch (e) {
    editError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteToolset(name: string) {
  try {
    await client.delete(`/api/toolsets/${name}`);
    await fetchToolsets();
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
      <div class="card" v-if="fetchedToolsets.length === 0">Aucun toolset.</div>
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-desc">Description</th>
            <th class="th-tools">Local tools</th>
            <th class="th-mcp">MCP servers</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in fetchedToolsets" :key="t.name">
            <td>{{ t.name }}</td>
            <td>{{ t.description ?? '—' }}</td>
            <td>
              {{ t.local_tools.length ? t.local_tools.join(', ') : '—' }}
            </td>
            <td>
              {{ t.mcp.length ? t.mcp.map((m) => m.server).join(', ') : '—' }}
            </td>
            <td>
              <button class="btn btn-edit" @click="startEdit(t)">Modifier</button>
              <button class="btn btn-delete" @click="deleteToolset(t.name)">Supprimer</button>
            </td>
          </tr>
        </tbody>
      </table>

      <div class="card form-card">
        <h3 class="form-title">Créer un toolset</h3>
        <label class="field">
          <span class="field-label">Nom</span>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-toolset"
            aria-label="Nom du toolset"
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
          <span class="field-label">Prompt</span>
          <textarea
            class="field-input"
            v-model="formPrompt"
            rows="3"
            placeholder="Prompt optionnel"
            aria-label="Prompt"
          />
        </label>
        <label class="field">
          <span class="field-label">Local tools</span>
          <div class="checkbox-list">
            <label v-for="t in localTools" :key="t.name" class="checkbox-item">
              <input type="checkbox" :value="t.name" v-model="formLocalTools" />
              <span>{{ t.name }}</span>
            </label>
          </div>
        </label>
        <label class="field">
          <span class="field-label">Serveurs MCP</span>
          <div v-for="(sel, i) in formMcp" :key="i" class="mcp-row">
            <select class="field-input" v-model="sel.server" aria-label="Serveur MCP" @change="onMcpServerChange(i)">
              <option value="">—</option>
              <option v-for="s in mcpServers" :key="s.name" :value="s.name">{{ s.name }}</option>
            </select>
            <div class="checkbox-list">
              <label v-for="tools in mcpToolsForServer(sel.server)" :key="tools" class="checkbox-item">
                <input type="checkbox" :value="tools" v-model="sel.tools" />
                <span>{{ tools }}</span>
              </label>
            </div>
            <p v-if="sel.server && mcpToolsForServer(sel.server).length === 0" class="empty-state">
              Aucun outil disponible — lancez un test sur ce serveur MCP.
            </p>
            <button class="btn btn-cancel" @click="removeMcpRow(i)">Retirer</button>
          </div>
          <button class="btn btn-add" @click="addMcpRow">Ajouter un serveur</button>
        </label>
        <div v-if="optionsError" class="creation-error">{{ optionsError }}</div>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <button class="btn btn-create" @click="createToolset">Créer</button>
      </div>

      <template v-for="t in fetchedToolsets" :key="'edit-' + t.name">
        <div v-if="t.name === editingName" class="card form-card">
          <h3 class="form-title">Modifier : {{ t.name }}</h3>
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
            <span class="field-label">Prompt</span>
            <textarea
              class="field-input"
              v-model="editPrompt"
              rows="3"
              placeholder="Prompt"
              aria-label="Prompt"
            />
          </label>
          <label class="field">
            <span class="field-label">Local tools</span>
            <div class="checkbox-list">
              <label v-for="t in localTools" :key="t.name" class="checkbox-item">
                <input type="checkbox" :value="t.name" v-model="editLocalTools" />
                <span>{{ t.name }}</span>
              </label>
            </div>
          </label>
          <label class="field">
            <span class="field-label">Serveurs MCP</span>
            <div v-for="(sel, i) in editMcp" :key="i" class="mcp-row">
              <select class="field-input" v-model="sel.server" aria-label="Serveur MCP" @change="onEditMcpServerChange(i)">
                <option value="">—</option>
                <option v-for="s in mcpServers" :key="s.name" :value="s.name">{{ s.name }}</option>
              </select>
              <div class="checkbox-list">
                <label v-for="tools in mcpToolsForServer(sel.server)" :key="tools" class="checkbox-item">
                  <input type="checkbox" :value="tools" v-model="sel.tools" />
                  <span>{{ tools }}</span>
                </label>
              </div>
              <p v-if="sel.server && mcpToolsForServer(sel.server).length === 0" class="empty-state">
                Aucun outil disponible — lancez un test sur ce serveur MCP.
              </p>
              <button class="btn btn-cancel" @click="removeEditMcpRow(i)">Retirer</button>
            </div>
            <button class="btn btn-add" @click="addEditMcpRow">Ajouter un serveur</button>
          </label>
          <div v-if="editError" class="creation-error">{{ editError }}</div>
          <div class="edit-actions">
            <button class="btn btn-success" @click="saveEdit(t.name)">Sauvegarder</button>
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
.th-tools,
.th-mcp,
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
.th-desc { width: 25%; }
.th-tools { width: 22%; }
.th-mcp { width: 18%; }
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

.btn-add {
  background: #2b3a4d;
  color: #6a7185;
  border: 1px dashed #3a4a5e;
  padding: 4px 10px;
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

.empty-state {
  color: #6a7185;
  font-size: 12px;
  margin: 4px 0;
}
</style>
