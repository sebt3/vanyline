<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import type { RunLaunch, WorkflowScope } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';

interface SandboxChoice {
  name: string;
  phase?: string;
}

const open = defineModel<boolean>('open', { required: true });

const props = withDefaults(
  defineProps<{
    project: string;
    workflow: { id: number; name: string };
    scope: WorkflowScope;
    /** Sandboxes du projet ; seules les `Running` sont proposées (portée sandbox). */
    sandboxes?: SandboxChoice[];
    /** Sandbox imposée par la vue appelante : pas de choix. */
    fixedSandbox?: string;
    client?: WorkflowsClient;
  }>(),
  {
    sandboxes: () => [],
    fixedSandbox: undefined,
    client: () => createWorkflowsClient(createApiClient()),
  },
);

const emit = defineEmits<{ launched: [run: RunLaunch] }>();

const runningSandboxes = computed(() =>
  props.sandboxes.filter((s) => s.phase === 'Running'),
);

const selected = ref('');
const busy = ref(false);
const error = ref<string | null>(null);

/** Sandbox cible du lancement ; `undefined` en portée projet. */
const targetSandbox = computed<string | undefined>(() => {
  if (props.scope !== 'sandbox') return undefined;
  return props.fixedSandbox ?? (selected.value || undefined);
});

const canLaunch = computed(
  () => !busy.value && (props.scope === 'project' || targetSandbox.value !== undefined),
);

watch(
  [open, runningSandboxes],
  ([isOpen]) => {
    if (!isOpen) return;
    error.value = null;
    selected.value = runningSandboxes.value[0]?.name ?? '';
  },
  { immediate: true },
);

function describeError(err: unknown): string {
  if (err instanceof EngineUnavailable) return 'Moteur de workflow indisponible.';
  return err instanceof ApiError ? err.message : String(err);
}

async function launch() {
  if (!canLaunch.value) return;
  busy.value = true;
  error.value = null;
  try {
    const run = await props.client.launchWorkflow(props.project, props.workflow.id, {
      sandbox: targetSandbox.value,
    });
    emit('launched', run);
    open.value = false;
  } catch (err) {
    error.value = describeError(err);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <DialogShell v-model:open="open" title="Lancer le workflow">
    <p class="launch-text">
      <strong>{{ workflow.name }}</strong>
      <template v-if="scope === 'project'"> — lancement au niveau du projet.</template>
    </p>

    <template v-if="scope === 'sandbox'">
      <p v-if="fixedSandbox" class="launch-text">Sandbox : {{ fixedSandbox }}</p>
      <Field v-else label="Sandbox">
        <select
          v-if="runningSandboxes.length > 0"
          v-model="selected"
          class="field-input"
          aria-label="Sandbox"
        >
          <option v-for="s in runningSandboxes" :key="s.name" :value="s.name">
            {{ s.name }}
          </option>
        </select>
        <span v-else class="launch-hint">Aucune sandbox en cours d'exécution.</span>
      </Field>
    </template>

    <div v-if="error" class="launch-error">{{ error }}</div>

    <template #actions>
      <button type="button" class="btn btn-primary" :disabled="!canLaunch" @click="launch">
        Lancer
      </button>
      <button type="button" class="btn" @click="open = false">Annuler</button>
    </template>
  </DialogShell>
</template>

<style scoped>
.launch-text {
  font-size: 13px;
  margin: 0 0 12px;
  color: #e6e9f0;
}

.launch-hint {
  font-size: 13px;
  color: #6a7185;
}

.launch-error {
  color: #e85d5d;
  font-size: 13px;
  margin-top: 8px;
}

.field-input {
  min-height: var(--ctl-h);
  width: 100%;
  padding: 0 10px;
  border-radius: 6px;
  border: 1px solid #2b2b4a;
  background: #0c1420;
  color: #e6e9f0;
}

.btn {
  min-height: var(--ctl-h);
  padding: 0 14px;
  border-radius: 6px;
  border: 1px solid #2b2b4a;
  background: #1c1c2a;
  color: #e6e9f0;
  cursor: pointer;
  font-size: 13px;
}

.btn:disabled {
  background: #2b3550;
  color: #6a7185;
  cursor: not-allowed;
}

.btn-primary {
  background: #4c90f0;
  border-color: #4c90f0;
  color: white;
}
</style>
