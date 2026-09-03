<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import type { McpServer, McpTransport } from '../ports';
import { useConfigRepo } from './useConfigRepo';
import { useCrudResource } from '../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';
import SourceBadge from '../common/SourceBadge.vue';

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const repo = useConfigRepo();
const resource = useCrudResource(repo, 'mcp');
const { items: fetchedServers, loading, error } = resource;

// Formulaire de création
const formName = ref('');
const formType = ref<McpTransport>('sse');
const formUrl = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingName = ref<string | null>(null); // nom d'origine (clé pour l'update)
const editName = ref('');
const editType = ref<McpTransport>('sse');
const editUrl = ref('');
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

onMounted(resource.fetch);

async function createServer() {
  creationError.value = null;
  try {
    await resource.create({ name: formName.value, type: formType.value, url: formUrl.value });
    formName.value = '';
    formType.value = 'sse';
    formUrl.value = '';
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = message(e);
  }
}

function startEdit(server: McpServer) {
  editingName.value = server.name;
  editName.value = server.name;
  editType.value = server.type;
  editUrl.value = server.url;
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editName.value = '';
  editType.value = 'sse';
  editUrl.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(originalName: string) {
  editError.value = null;
  try {
    await resource.update(originalName, {
      name: editName.value,
      type: editType.value,
      url: editUrl.value,
    });
    cancelEdit();
  } catch (e) {
    editError.value = message(e);
  }
}

async function deleteServer(name: string) {
  await resource.remove(name);
}

// Découverte des tools : par serveur, un état de chargement/erreur dédié —
// tester un serveur ne doit pas bloquer/masquer les autres lignes.
const discovering = ref<Record<string, boolean>>({});
const discoverError = ref<Record<string, string | null>>({});

async function discoverTools(name: string) {
  discovering.value = { ...discovering.value, [name]: true };
  discoverError.value = { ...discoverError.value, [name]: null };
  try {
    const result = await repo.testMcpServer(name);
    const server = fetchedServers.value.find((s) => s.name === name);
    if (server) server.available_tools = result.tools;
  } catch (e) {
    discoverError.value = { ...discoverError.value, [name]: message(e) };
  } finally {
    discovering.value = { ...discovering.value, [name]: false };
  }
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedServers.length === 0" message="Aucun serveur MCP." />
      <table class="table" v-else>
        <thead>
          <tr>
            <th class="th-name">Nom</th>
            <th class="th-type">Type</th>
            <th class="th-url">URL</th>
            <th class="th-tools">Tools</th>
            <th class="th-actions"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="s in fetchedServers" :key="s.name">
            <td>{{ s.name }} <SourceBadge :source="s.source" /></td>
            <td>{{ s.type }}</td>
            <td>{{ s.url }}</td>
            <td>
              <span
                v-if="s.available_tools?.length"
                class="tools-list"
                :title="s.available_tools.join(', ')"
              >
                {{ s.available_tools.length }} tool{{ s.available_tools.length > 1 ? 's' : '' }}
              </span>
              <span v-else class="tools-empty">jamais testé</span>
              <div v-if="discoverError[s.name]" class="discover-error">{{ discoverError[s.name] }}</div>
            </td>
            <td class="th-actions">
              <button
                class="btn btn-discover"
                :disabled="discovering[s.name]"
                @click="discoverTools(s.name)"
              >
                {{ discovering[s.name] ? 'Découverte…' : 'Découvrir' }}
              </button>
              <button class="btn btn-edit" @click="startEdit(s)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteServer(s.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un serveur MCP</button>

      <DialogShell v-model:open="createModalOpen" title="Créer un serveur MCP">
        <Field label="Nom" top-align>
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-mcp-server"
            aria-label="Nom du serveur"
          />
        </Field>
        <Field label="Type" top-align>
          <select
            class="field-input"
            v-model="formType"
            aria-label="Type de serveur"
          >
            <option value="sse">sse</option>
            <option value="http-streamable">http-streamable</option>
          </select>
        </Field>
        <Field label="URL" top-align>
          <input
            class="field-input"
            v-model="formUrl"
            type="text"
            placeholder="https://example.com/mcp"
            aria-label="URL"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createServer">Créer</button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>

      <DialogShell v-model:open="editModalOpen" :title="`Modifier : ${editingName}`">
        <Field label="Nom" top-align>
          <input
            class="field-input"
            v-model="editName"
            type="text"
            placeholder="Nom du serveur"
            aria-label="Nom du serveur"
          />
        </Field>
        <Field label="Type" top-align>
          <select
            class="field-input"
            v-model="editType"
            aria-label="Type de serveur"
          >
            <option value="sse">sse</option>
            <option value="http-streamable">http-streamable</option>
          </select>
        </Field>
        <Field label="URL" top-align>
          <input
            class="field-input"
            v-model="editUrl"
            type="text"
            placeholder="https://example.com/mcp"
            aria-label="URL"
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
  max-width: 900px;
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
  width: 22%;
}

.th-type {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 14%;
}

.th-url {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 30%;
}

.th-tools {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
  width: 14%;
}

.tools-list {
  color: #3fb56d;
  font-size: 13px;
}

.tools-empty {
  color: #6a7185;
  font-size: 13px;
}

.discover-error {
  margin-top: 4px;
  color: #ff9db3;
  font-size: 11px;
}

.btn-discover {
  background: #2b3550;
  color: #9db4f0;
  border: 1px solid #3a4570;
}

.btn-discover:hover {
  background: #354168;
}

.btn-discover:disabled {
  color: #6a7185;
  cursor: not-allowed;
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
  margin-left: 6px;
}
.btn:first-child {
  margin-left: 0;
}
</style>
