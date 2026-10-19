<script setup lang="ts">
import { computed, inject, ref, unref, type Ref } from 'vue';
import type { RunLaunch, WorkflowSummary } from '@vanyline/protocol';
import { createApiClient } from '../../api/client';
import { createWorkflowsClient } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import { useSandboxState } from '../../composables/useSandboxState';
import LaunchDialog from '../workflows/LaunchDialog.vue';
import RunDetail from '../workflows/RunDetail.vue';
import RunList from '../workflows/RunList.vue';
import WorkflowList from '../workflows/WorkflowList.vue';

// Panneau « Workflows » de l'IDE (ide.sdd) : workflows de portée sandbox du projet,
// lancement sur la sandbox courante, runs de CETTE sandbox. Aucune donnée de maquette.
// Fourni par IdeShell sous forme de ref (réactif au changement de projet) ; chaîne acceptée.
const injectedProject = inject<Ref<string> | string>('project-name', '');
const projectName = computed(() => unref(injectedProject));
const sandboxName = inject<string>('sandbox-name', '');

const { phases } = useSandboxState();
const phase = computed(() => phases.value.get(sandboxName) ?? null);

const client: WorkflowsClient = createWorkflowsClient(createApiClient());

/** Lancement seulement sur une sandbox `Running` (ide.sdd : lancer sur la sandbox courante). */
const running = computed(() => phase.value === 'Running');

const launchTarget = ref<WorkflowSummary | null>(null);
const launchOpen = ref(false);
/** Incrémenté après un lancement : force le rechargement de la liste des runs. */
const runsKey = ref(0);
/** Run ouvert dans le détail ; `null` = liste des runs. */
const openedRunId = ref<string | null>(null);

const sandboxChoices = computed(() => [
  { name: sandboxName, phase: phase.value ?? undefined },
]);

function askLaunch(wf: WorkflowSummary) {
  if (!running.value) return;
  launchTarget.value = wf;
  launchOpen.value = true;
}

function onLaunched(_run: RunLaunch) {
  runsKey.value += 1;
}
</script>

<template>
  <div class="workflow-panel">
    <section v-if="openedRunId === null" class="section">
      <h3 class="section-title">Workflows de la sandbox</h3>
      <p v-if="!running" class="notice">
        Sandbox non démarrée (phase : {{ phase ?? 'inconnue' }}) : lancement désactivé.
      </p>
      <WorkflowList
        :project="projectName"
        :client="client"
        readonly
        fixed-scope="sandbox"
        @launch="askLaunch"
      />
    </section>

    <section v-if="openedRunId === null" class="section">
      <h3 class="section-title">Runs de cette sandbox</h3>
      <RunList
        :key="runsKey"
        :project="projectName"
        :sandbox="sandboxName"
        @select="(id: string) => (openedRunId = id)"
      />
    </section>

    <section v-else class="section">
      <button type="button" class="back" @click="openedRunId = null">← Retour aux runs</button>
      <RunDetail :run-id="openedRunId" />
    </section>

    <LaunchDialog
      v-if="launchTarget"
      v-model:open="launchOpen"
      :project="projectName"
      :workflow="launchTarget"
      scope="sandbox"
      :sandboxes="sandboxChoices"
      :fixed-sandbox="sandboxName"
      @launched="onLaunched"
    />
  </div>
</template>

<style scoped>
.workflow-panel {
  height: 100%;
  overflow: auto;
  padding: 12px;
  box-sizing: border-box;
  background: var(--dv-group-view-background-color);
  color: var(--dv-color-abyss-secondary-text);
  font-size: 12px;
}
.section { margin-bottom: 16px; }
.notice { margin: 0 0 8px; color: #e0a83d; }
.section-title {
  margin: 0 0 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--dv-color-abyss-primary-text);
}
.back {
  margin-bottom: 8px;
  background: none;
  border: none;
  color: var(--dv-color-abyss-secondary-text);
  cursor: pointer;
  font-size: 12px;
}
</style>
