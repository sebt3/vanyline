<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import type { McpSelection, McpServer, Toolset } from '../ports';
import { useConfigRepo } from './useConfigRepo';
import { useCrudResource } from '../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import CheckboxList from '../common/CheckboxList.vue';
import Field from '../common/Field.vue';

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** État local d'une ligne MCP du formulaire : `tools` toujours initialisé
 *  (contrairement à `McpSelection`, où il est optionnel). */
interface McpRow {
  server: string;
  tools: string[];
}

const repo = useConfigRepo();
const resource = useCrudResource(repo, 'toolsets');
const { items: fetchedToolsets, loading, error } = resource;

const localTools = ref<string[]>([]);
const mcpServers = ref<McpServer[]>([]);
const optionsError = ref<string | null>(null);

const formName = ref('');
const formDescription = ref('');
const formPrompt = ref('');
const formLocalTools = ref<string[]>([]);
const formMcp = ref<McpRow[]>([]);
const creationError = ref<string | null>(null);

const editingName = ref<string | null>(null);
const editDescription = ref('');
const editPrompt = ref('');
const editLocalTools = ref<string[]>([]);
const editMcp = ref<McpRow[]>([]);
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchOptions() {
  try {
    const [lt, servers] = await Promise.all([repo.listLocalTools(), repo.list('mcp')]);
    localTools.value = lt;
    mcpServers.value = servers;
  } catch (e) {
    optionsError.value = message(e);
  }
}

function mcpToolsForServer(server: string): string[] {
  return mcpServers.value.find((s) => s.name === server)?.available_tools ?? [];
}

function toOptions(names: string[]): { value: string; label: string }[] {
  return names.map((name) => ({ value: name, label: name }));
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
  resource.fetch();
  fetchOptions();
});

async function createToolset() {
  creationError.value = null;
  try {
    await resource.create({
      name: formName.value,
      ...(formDescription.value ? { description: formDescription.value } : {}),
      ...(formPrompt.value ? { prompt: formPrompt.value } : {}),
      local_tools: formLocalTools.value,
      mcp: formMcp.value as McpSelection[],
    });
    formName.value = '';
    formDescription.value = '';
    formPrompt.value = '';
    formLocalTools.value = [];
    formMcp.value = [];
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = message(e);
  }
}

function startEdit(toolset: Toolset) {
  editingName.value = toolset.name;
  editDescription.value = toolset.description ?? '';
  editPrompt.value = toolset.prompt ?? '';
  editLocalTools.value = [...toolset.local_tools];
  editMcp.value = toolset.mcp.map((m) => ({ server: m.server, tools: m.tools ?? [] }));
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editDescription.value = '';
  editPrompt.value = '';
  editLocalTools.value = [];
  editMcp.value = [];
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(name: string) {
  editError.value = null;
  const patch: Partial<Toolset> = {};
  if (editDescription.value) patch.description = editDescription.value;
  if (editPrompt.value) patch.prompt = editPrompt.value;
  if (editLocalTools.value.length > 0) patch.local_tools = editLocalTools.value;
  if (editMcp.value.length > 0) patch.mcp = editMcp.value as McpSelection[];
  try {
    await resource.update(name, patch);
    cancelEdit();
  } catch (e) {
    editError.value = message(e);
  }
}

async function deleteToolset(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedToolsets.length === 0" message="Aucun toolset." />
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

      <button class="btn btn-create" @click="createModalOpen = true">Créer un toolset</button>

      <ErrorCard v-if="optionsError" :message="optionsError" />

      <DialogShell v-model:open="createModalOpen" title="Créer un toolset">
        <Field label="Nom" top-align>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-toolset"
            aria-label="Nom du toolset"
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
        <Field label="Prompt" top-align>
          <textarea
            class="field-input"
            v-model="formPrompt"
            rows="3"
            placeholder="Prompt optionnel"
            aria-label="Prompt"
          />
        </Field>
        <Field label="Local tools" top-align>
          <CheckboxList :options="toOptions(localTools)" v-model="formLocalTools" />
        </Field>
        <Field label="Serveurs MCP" top-align>
          <div v-for="(sel, i) in formMcp" :key="i" class="mcp-row">
            <select class="field-input" v-model="sel.server" aria-label="Serveur MCP" @change="onMcpServerChange(i)">
              <option value="">—</option>
              <option v-for="s in mcpServers" :key="s.name" :value="s.name">{{ s.name }}</option>
            </select>
            <CheckboxList :options="toOptions(mcpToolsForServer(sel.server))" v-model="sel.tools" />
            <p v-if="sel.server && mcpToolsForServer(sel.server).length === 0" class="empty-state">
              Aucun outil disponible — lancez un test sur ce serveur MCP.
            </p>
            <button class="btn btn-cancel" @click="removeMcpRow(i)">Retirer</button>
          </div>
          <button class="btn btn-add" @click="addMcpRow">Ajouter un serveur</button>
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createToolset">Créer</button>
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
        <Field label="Prompt" top-align>
          <textarea
            class="field-input"
            v-model="editPrompt"
            rows="3"
            placeholder="Prompt"
            aria-label="Prompt"
          />
        </Field>
        <Field label="Local tools" top-align>
          <CheckboxList :options="toOptions(localTools)" v-model="editLocalTools" />
        </Field>
        <Field label="Serveurs MCP" top-align>
          <div v-for="(sel, i) in editMcp" :key="i" class="mcp-row">
            <select class="field-input" v-model="sel.server" aria-label="Serveur MCP" @change="onEditMcpServerChange(i)">
              <option value="">—</option>
              <option v-for="s in mcpServers" :key="s.name" :value="s.name">{{ s.name }}</option>
            </select>
            <CheckboxList :options="toOptions(mcpToolsForServer(sel.server))" v-model="sel.tools" />
            <p v-if="sel.server && mcpToolsForServer(sel.server).length === 0" class="empty-state">
              Aucun outil disponible — lancez un test sur ce serveur MCP.
            </p>
            <button class="btn btn-cancel" @click="removeEditMcpRow(i)">Retirer</button>
          </div>
          <button class="btn btn-add" @click="addEditMcpRow">Ajouter un serveur</button>
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
  margin-left: 6px;
}
.btn:first-child {
  margin-left: 0;
}

.btn-add {
  background: #2b3a4d;
  color: #6a7185;
  border: 1px dashed #3a4a5e;
  padding: 4px 10px;
}

.empty-state {
  color: #6a7185;
  font-size: 12px;
  margin: 4px 0;
}
</style>
