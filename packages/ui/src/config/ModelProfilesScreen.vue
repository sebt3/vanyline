<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { DialogClose } from 'reka-ui';
import type { ModelProfile, Provider } from '../ports';
import { useConfigRepo } from './useConfigRepo';
import { useCrudResource } from '../composables/useCrudResource';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Une ligne du petit éditeur clé/valeur pour `options` — les noms de
 *  paramètres (top_p, top_k, num_predict, thinking_mode...) varient trop
 *  selon le backend LLM pour figer une liste de champs typés. */
interface OptionRow {
  key: string;
  value: string;
}

/** `raw` tenté en JSON (nombre, booléen, objet...) ; repli sur la chaîne
 *  brute si `raw` n'est pas du JSON valide. */
function parseOptionValue(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

/** `undefined` si aucune ligne n'a de clé non vide — pour ne jamais envoyer
 *  `options: {}` alors que l'utilisateur n'a rien renseigné. */
function optionsFromRows(rows: OptionRow[]): Record<string, unknown> | undefined {
  const out: Record<string, unknown> = {};
  for (const { key, value } of rows) {
    const trimmedKey = key.trim();
    if (!trimmedKey) continue;
    out[trimmedKey] = parseOptionValue(value);
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

function rowsFromOptions(options?: Record<string, unknown>): OptionRow[] {
  if (!options) return [];
  return Object.entries(options).map(([key, value]) => ({
    key,
    value: typeof value === 'string' ? value : JSON.stringify(value),
  }));
}

const repo = useConfigRepo();
const resource = useCrudResource(repo, 'profiles');
const { items: fetchedProfiles, loading, error } = resource;

const providers = ref<Provider[]>([]);
const providersError = ref<string | null>(null);

// Formulaire de création
const formName = ref('');
const formProvider = ref('');
const formModel = ref('');
const formTemperature = ref('');
const formMaxTokens = ref('');
const formOptions = ref<OptionRow[]>([]);
const formAvailableModels = ref<string[]>([]);
const creationError = ref<string | null>(null);

// Formulaire d'édition
const editingName = ref<string | null>(null);
const editProvider = ref('');
const editModel = ref('');
const editAvailableModels = ref<string[]>([]);
const editTemperature = ref('');
const editMaxTokens = ref('');
const editOptions = ref<OptionRow[]>([]);
const editError = ref<string | null>(null);

// Modales
const createModalOpen = ref(false);
const editModalOpen = ref(false);

async function fetchProviders() {
  try {
    providers.value = await repo.list('providers');
  } catch (e) {
    // Erreur de chargement des providers consignée mais ne bloque pas
    // l'affichage des profils existants.
    providersError.value = message(e);
  }
}

/** Lorsque les providers sont chargés (après startEdit), pré-remplir les modèles. */
watch(
  () => providers.value,
  () => {
    if (editProvider.value) {
      editAvailableModels.value = modelsForProvider(editProvider.value);
    }
  },
  { immediate: false },
);

onMounted(async () => {
  await resource.fetch();
  await fetchProviders();
});

/** `available_models` du provider donné par nom (dépendance du select modèle). */
function modelsForProvider(providerName: string): string[] {
  return providers.value.find((p) => p.name === providerName)?.available_models ?? [];
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
  const options = optionsFromRows(formOptions.value);
  try {
    await resource.create({
      name: formName.value,
      provider: formProvider.value,
      model: formModel.value,
      ...(formTemperature.value ? { temperature: Number(formTemperature.value) } : {}),
      ...(formMaxTokens.value ? { max_tokens: Number(formMaxTokens.value) } : {}),
      ...(options ? { options } : {}),
    });
    formName.value = '';
    formProvider.value = '';
    formModel.value = '';
    formTemperature.value = '';
    formMaxTokens.value = '';
    formOptions.value = [];
    createModalOpen.value = false;
  } catch (e) {
    creationError.value = message(e);
  }
}

function startEdit(profile: ModelProfile) {
  editingName.value = profile.name;
  editProvider.value = profile.provider;
  editAvailableModels.value = modelsForProvider(profile.provider);
  editModel.value = profile.model;
  editTemperature.value = profile.temperature?.toString() ?? '';
  editMaxTokens.value = profile.max_tokens?.toString() ?? '';
  editOptions.value = rowsFromOptions(profile.options);
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
  editOptions.value = [];
  editError.value = null;
  editModalOpen.value = false;
}

async function saveEdit(name: string) {
  editError.value = null;
  const options = optionsFromRows(editOptions.value);
  try {
    await resource.update(name, {
      ...(editProvider.value ? { provider: editProvider.value } : {}),
      ...(editModel.value ? { model: editModel.value } : {}),
      ...(editTemperature.value ? { temperature: Number(editTemperature.value) } : {}),
      ...(editMaxTokens.value ? { max_tokens: Number(editMaxTokens.value) } : {}),
      ...(options ? { options } : {}),
    });
    cancelEdit();
  } catch (e) {
    editError.value = message(e);
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
        <Field label="Options avancées (optionnel)" top-align>
          <div class="options-editor">
            <p class="options-hint">
              top_p, top_k, min_p, repeat_penalty, thinking_mode... — dépend du backend LLM.
            </p>
            <div v-for="(row, idx) in formOptions" :key="idx" class="option-row">
              <input
                class="field-input option-key"
                v-model="row.key"
                type="text"
                placeholder="top_p"
                :aria-label="`Option ${idx + 1} clé`"
              />
              <input
                class="field-input option-value"
                v-model="row.value"
                type="text"
                placeholder="0.9"
                :aria-label="`Option ${idx + 1} valeur`"
              />
              <button
                class="btn btn-delete option-remove"
                type="button"
                aria-label="Supprimer cette option"
                @click="formOptions.splice(idx, 1)"
              >
                ×
              </button>
            </div>
            <button
              class="btn option-add"
              type="button"
              @click="formOptions.push({ key: '', value: '' })"
            >
              + Ajouter une option
            </button>
          </div>
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
        <Field label="Options avancées" top-align>
          <div class="options-editor">
            <p class="options-hint">
              top_p, top_k, min_p, repeat_penalty, thinking_mode... — dépend du backend LLM.
            </p>
            <div v-for="(row, idx) in editOptions" :key="idx" class="option-row">
              <input
                class="field-input option-key"
                v-model="row.key"
                type="text"
                placeholder="top_p"
                :aria-label="`Option ${idx + 1} clé`"
              />
              <input
                class="field-input option-value"
                v-model="row.value"
                type="text"
                placeholder="0.9"
                :aria-label="`Option ${idx + 1} valeur`"
              />
              <button
                class="btn btn-delete option-remove"
                type="button"
                aria-label="Supprimer cette option"
                @click="editOptions.splice(idx, 1)"
              >
                ×
              </button>
            </div>
            <button
              class="btn option-add"
              type="button"
              @click="editOptions.push({ key: '', value: '' })"
            >
              + Ajouter une option
            </button>
          </div>
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

.options-editor {
  width: 100%;
}

.options-hint {
  color: #6a7185;
  font-size: 11px;
  margin: 0 0 8px 0;
}

.option-row {
  display: flex;
  gap: 6px;
  margin-bottom: 6px;
}

.option-key {
  flex: 1;
}

.option-value {
  flex: 1;
}

.option-remove {
  flex: none;
  width: 28px;
}

.option-add {
  margin-left: 0;
}
</style>
