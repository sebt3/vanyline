<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ApiError, createApiClient } from '../../api/client';
import type { ApiClient } from '../../api/client';
import {
  getDefaultModel,
  listProviderOptions,
  saveDefaultModel,
  type DefaultModelValue,
  type ProviderOption,
} from '../../api/appSettingsClient';

const props = withDefaults(defineProps<{ client?: ApiClient }>(), {
  client: () => createApiClient(),
});

const providers = ref<ProviderOption[]>([]);
const providerId = ref('');
const model = ref('');
const optionsText = ref('');
const current = ref<{ id: number } | null>(null);
const loadError = ref<string | null>(null);
const error = ref<string | null>(null);
const saved = ref(false);
const saving = ref(false);

function describe(err: unknown): string {
  return err instanceof ApiError ? err.message : String(err);
}

async function load() {
  try {
    providers.value = await listProviderOptions(props.client);
    const existing = await getDefaultModel(props.client);
    current.value = existing ? { id: existing.id } : null;
    if (existing) {
      providerId.value = String(existing.value.provider_id);
      model.value = existing.value.model;
      optionsText.value = existing.value.options ? JSON.stringify(existing.value.options) : '';
    }
  } catch (err) {
    loadError.value = describe(err);
  }
}

/** Options facultatives : un objet JSON, ou rien. */
function parseOptions(raw: string): { ok: true; options?: Record<string, unknown> } | { ok: false } {
  const text = raw.trim();
  if (!text) return { ok: true };
  try {
    const parsed: unknown = JSON.parse(text);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { ok: false };
    return { ok: true, options: parsed as Record<string, unknown> };
  } catch {
    return { ok: false };
  }
}

async function save() {
  error.value = null;
  saved.value = false;
  const name = model.value.trim();
  if (!providerId.value) {
    error.value = 'Choisis un fournisseur.';
    return;
  }
  if (!name) {
    error.value = 'Le nom du modèle est requis.';
    return;
  }
  const opts = parseOptions(optionsText.value);
  if (!opts.ok) {
    error.value = 'Options : un objet JSON est attendu (JSON invalide).';
    return;
  }
  const value: DefaultModelValue = {
    provider_id: Number(providerId.value),
    model: name,
    ...(opts.options ? { options: opts.options } : {}),
  };
  saving.value = true;
  try {
    await saveDefaultModel(current.value, value, props.client);
    const existing = await getDefaultModel(props.client);
    current.value = existing ? { id: existing.id } : null;
    saved.value = true;
  } catch (err) {
    error.value = describe(err);
  } finally {
    saving.value = false;
  }
}

onMounted(load);
</script>

<template>
  <section class="global-settings">
    <h2>Modèle par défaut</h2>
    <p class="global-settings__hint">
      Modèle utilisé pour amorcer les agents système des utilisateurs qui n'ont encore aucun profil
      de modèle.
    </p>
    <p v-if="loadError" class="global-settings__error" role="alert">{{ loadError }}</p>

    <label class="global-settings__field">
      <span>Fournisseur global</span>
      <select aria-label="Fournisseur global" v-model="providerId">
        <option value="">— choisir —</option>
        <option v-for="p in providers" :key="p.id" :value="String(p.id)">{{ p.name }}</option>
      </select>
    </label>
    <label class="global-settings__field">
      <span>Nom du modèle</span>
      <input aria-label="Nom du modèle" v-model="model" type="text" />
    </label>
    <label class="global-settings__field">
      <span>Options (JSON, facultatif)</span>
      <textarea aria-label="Options (JSON)" v-model="optionsText" rows="4"></textarea>
    </label>

    <p v-if="error" class="global-settings__error" role="alert">{{ error }}</p>
    <p v-if="saved" class="global-settings__ok" role="status">Enregistré.</p>
    <button type="button" class="global-settings__save" :disabled="saving" @click="save">
      Enregistrer
    </button>
  </section>
</template>

<style scoped>
.global-settings {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  max-width: 640px;
}

.global-settings__hint {
  font-size: 13px;
  color: #6a7185;
  margin: 0;
}

.global-settings__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 13px;
  min-width: 0;
}

.global-settings__field input,
.global-settings__field select,
.global-settings__field textarea {
  min-height: var(--ctl-h);
  max-width: 100%;
}

.global-settings__save {
  min-height: var(--ctl-h);
  align-self: flex-start;
  padding: 0 16px;
}

.global-settings__error {
  color: #e06c75;
  font-size: 13px;
  margin: 0;
}

.global-settings__ok {
  color: #3fb56d;
  font-size: 13px;
  margin: 0;
}
</style>
