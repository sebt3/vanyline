<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { DialogClose } from 'reka-ui';
import type { Provider, ProviderType } from '../ports';
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
const resource = useCrudResource(repo, 'providers');
const { items: fetchedProviders, loading, error } = resource;

// Formulaire de création
const formName = ref('');
const formProviderType = ref<ProviderType>('ollama');
const formEndpoint = ref('');
const formApiKey = ref('');
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingName = ref<string | null>(null); // nom d'origine (clé pour l'update)
const editName = ref('');
const editProviderType = ref<ProviderType>('ollama');
const editEndpoint = ref('');
const editApiKey = ref('');
const editError = ref<string | null>(null);

// Résultat du test — keyé par nom
const testResults = ref<Record<string, string>>({});

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

onMounted(resource.fetch);

async function createProvider() {
  creationError.value = null;
  try {
    await resource.create({
      name: formName.value,
      type: formProviderType.value,
      endpoint: formEndpoint.value,
      ...(formApiKey.value ? { api_key: formApiKey.value } : {}),
    });
    formName.value = '';
    formProviderType.value = 'ollama';
    formEndpoint.value = '';
    formApiKey.value = '';
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = message(e);
  }
}

function startEdit(provider: Provider) {
  editingName.value = provider.name;
  editName.value = provider.name;
  editProviderType.value = provider.type;
  editEndpoint.value = provider.endpoint;
  editApiKey.value = provider.api_key ?? '';
  editError.value = null;
  editModalOpen.value = true;
}

function cancelEdit() {
  editingName.value = null;
  editName.value = '';
  editProviderType.value = 'ollama';
  editEndpoint.value = '';
  editApiKey.value = '';
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(originalName: string) {
  editError.value = null;
  try {
    await resource.update(originalName, {
      name: editName.value,
      type: editProviderType.value,
      endpoint: editEndpoint.value,
      ...(editApiKey.value ? { api_key: editApiKey.value } : {}),
    });
    cancelEdit();
  } catch (e) {
    editError.value = message(e);
  }
}

async function testProvider(name: string) {
  try {
    const result = await repo.testProvider(name);
    testResults.value[name] = result.models.join(', ');
  } catch (e) {
    testResults.value[name] = message(e);
  }
}

async function setDefault(name: string) {
  try {
    await repo.setDefaultProvider(name);
    await resource.fetch();
  } catch (e) {
    error.value = message(e);
  }
}

async function deleteProvider(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <LoadingSkeleton v-if="loading" />
  <div v-else>
    <ErrorCard v-if="error" :message="error" />
    <div v-else>
      <EmptyState v-if="fetchedProviders.length === 0" message="Aucun fournisseur LLM." />
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
          <tr v-for="p in fetchedProviders" :key="p.name">
            <td>{{ p.name }} <SourceBadge :source="p.source" /></td>
            <td class="th-type">{{ p.type }}</td>
            <td class="th-endpoint">{{ p.endpoint }}</td>
            <td class="th-status">
              <span v-if="p.is_default" class="badge-default">Défaut</span>
            </td>
            <td class="th-actions">
              <button class="btn btn-test" @click="testProvider(p.name)">
                Tester
              </button>
              <button class="btn btn-default" :class="{ 'btn-default-active': p.is_default }" @click="setDefault(p.name)">
                Défaut
              </button>
              <button class="btn btn-edit" @click="startEdit(p)">
                Modifier
              </button>
              <button class="btn btn-delete" @click="deleteProvider(p.name)">
                Supprimer
              </button>
            </td>
          </tr>
        </tbody>
      </table>

      <button class="btn btn-create" @click="createModalOpen = true">Créer un fournisseur</button>

      <DialogShell v-model:open="createModalOpen" title="Créer un fournisseur">
        <Field label="Nom">
          <input
            class="field-input"
            v-model="formName"
            type="text"
            placeholder="mon-fournisseur"
            aria-label="Nom du fournisseur"
          />
        </Field>
        <Field label="Type">
          <select
            class="field-input"
            v-model="formProviderType"
            aria-label="Type de fournisseur"
          >
            <option value="ollama">ollama</option>
            <option value="openai-compatible">openai-compatible</option>
          </select>
        </Field>
        <Field label="Endpoint">
          <input
            class="field-input"
            v-model="formEndpoint"
            type="text"
            placeholder="http://localhost:11434"
            aria-label="Endpoint"
          />
        </Field>
        <Field label="Clé API (optionnel)">
          <input
            class="field-input"
            v-model="formApiKey"
            type="text"
            placeholder="sk-..."
            aria-label="Clé API"
          />
        </Field>
        <div v-if="creationError" class="creation-error">{{ creationError }}</div>
        <template #actions>
          <button class="btn btn-create" @click="createProvider">Créer</button>
          <DialogClose class="btn btn-cancel">Annuler</DialogClose>
        </template>
      </DialogShell>

      <DialogShell v-model:open="editModalOpen" :title="`Modifier : ${editingName}`">
        <Field label="Nom">
          <input
            class="field-input"
            v-model="editName"
            type="text"
            placeholder="nom"
            aria-label="Nom"
          />
        </Field>
        <Field label="Type">
          <select
            class="field-input"
            v-model="editProviderType"
            aria-label="Type de fournisseur"
          >
            <option value="ollama">ollama</option>
            <option value="openai-compatible">openai-compatible</option>
          </select>
        </Field>
        <Field label="Endpoint">
          <input
            class="field-input"
            v-model="editEndpoint"
            type="text"
            placeholder="http://localhost:11434"
            aria-label="Endpoint"
          />
        </Field>
        <Field label="Clé API (optionnel)">
          <input
            class="field-input"
            v-model="editApiKey"
            type="text"
            placeholder="sk-..."
            aria-label="Clé API"
          />
        </Field>
        <div v-if="editError" class="creation-error">{{ editError }}</div>
        <template #actions>
          <button class="btn btn-success" @click="saveEdit(editingName!)">Sauvegarder</button>
          <DialogClose class="btn btn-cancel" @click="cancelEdit">Annuler</DialogClose>
        </template>
      </DialogShell>

      <div v-for="p in fetchedProviders" :key="'test-' + p.name" class="results">
        <div v-if="testResults[p.name]" class="test-result">
          Résultat pour {{ p.name }} : {{ testResults[p.name] }}
        </div>
      </div>
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

.btn {
  margin-left: 6px;
}
.btn:first-child {
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
