<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import { useRouter } from 'vue-router';
import { DialogClose } from 'reka-ui';
import type { RunLaunch, Workflow, WorkflowCreate, WorkflowSummary, WorkflowUpdate } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import { useCrudResource } from '../../composables/useCrudResource';
import { useSandboxState, registerRefresh, unregisterRefresh } from '../../composables/useSandboxState';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import ErrorCard from '../common/ErrorCard.vue';
import EmptyState from '../common/EmptyState.vue';
import DialogShell from '../common/DialogShell.vue';
import Field from '../common/Field.vue';
import ContextChat from '../ContextChat.vue';
import WorkflowList from '../workflows/WorkflowList.vue';
import WorkflowEditor from '../workflows/WorkflowEditor.vue';
import LaunchDialog from '../workflows/LaunchDialog.vue';
import RunList from '../workflows/RunList.vue';
import { LG_MIN_WIDTH, MD_MIN_WIDTH, useViewportWidth } from './viewport';

interface Toolchain {
  name: string;
  image: string;
}

interface SandboxSpec {
  project: string;
  branch: string;
  toolchains?: Toolchain[];
  resources?: unknown;
  suspended?: boolean;
}

interface Sandbox {
  metadata: { name: string };
  spec: SandboxSpec;
  status?: { phase?: string | null };
}

interface CreateSandboxBody {
  name: string;
  project: string;
  branch: string;
}

interface Project {
  metadata: { name: string };
  status?: { cloned?: boolean } | null;
}

const props = defineProps<{ projectName: string }>();

const client = createApiClient();
const router = useRouter();
const resource = useCrudResource<Sandbox>(client, '/api/sandboxes');
const { items: fetchedSandboxes, loading, error } = resource;
useSandboxState();

/** Le clone initial du Project doit être terminé avant de pouvoir créer une
 *  sandbox dessus (le worktree n'existe pas encore sinon). `null` tant que
 *  le Project n'est pas chargé — traité comme "pas prêt" (bouton désactivé
 *  par défaut, jamais un flash de faux-positif). */
const project = ref<Project | null>(null);
const projectReady = computed(() => project.value?.status?.cloned === true);

onMounted(() => {
  resource.fetch();
  // Le hub sandbox-state (WS temps réel) refetch le listing sur tout changement
  // de phase — cf. `useSandboxState`.
  registerRefresh(resource.fetch);
  fetchProject();
});

onBeforeUnmount(() => {
  unregisterRefresh(resource.fetch);
});

async function fetchProject() {
  try {
    project.value = await client.get<Project>(`/api/projects/${encodeURIComponent(props.projectName)}`);
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

/** Sandboxes du projet courant, filtrées côté client (fetch global /api/sandboxes). */
const projectSandboxes = computed(() =>
  fetchedSandboxes.value.filter((s) => s.spec.project === props.projectName),
);

/** Largeur de fenêtre : l'IDE exige `lg` ; sous `md` les assistants sont en feuille. */
const width = useViewportWidth();
const ideAvailable = computed(() => width.value >= LG_MIN_WIDTH);
const stacked = computed(() => width.value < MD_MIN_WIDTH);

function isRunning(sandbox: Sandbox): boolean {
  return sandbox.status?.phase === 'Running';
}

/** IDE : seulement si la sandbox tourne ET que la fenêtre est assez large. */
function openIde(sandbox: Sandbox) {
  if (!isRunning(sandbox) || !ideAvailable.value) return;
  router.push(`/p/${encodeURIComponent(props.projectName)}/s/${encodeURIComponent(sandbox.metadata.name)}`);
}

/** Vue d'ensemble responsive : toujours disponible. */
function openOverview(sandbox: Sandbox) {
  router.push(`/p/${encodeURIComponent(props.projectName)}/s/${encodeURIComponent(sandbox.metadata.name)}/overview`);
}

/** Clic sur la ligne : IDE si possible, sinon vue d'ensemble (sandbox prête seulement). */
function onRowClick(sandbox: Sandbox) {
  if (!isRunning(sandbox)) return;
  if (ideAvailable.value) openIde(sandbox);
  else openOverview(sandbox);
}

/** Titre du bouton IDE : pourquoi il est désactivé, sinon rien. */
function ideTitle(sandbox: Sandbox): string | undefined {
  if (!isRunning(sandbox)) return "La sandbox n'est pas encore prête";
  if (!ideAvailable.value) return "L'IDE demande une fenêtre d'au moins 1024 px";
  return undefined;
}

// --- Workflows ----------------------------------------------------------------

const wfClient = createWorkflowsClient(client);
/** Incrémenté après une mutation : remonte WorkflowList et RunList (rechargement). */
const sectionsVersion = ref(0);
const actionError = ref<string | null>(null);

const editorOpen = ref(false);
const editingWorkflow = ref<Workflow | null>(null);
const editorSaving = ref(false);
const editorError = ref<unknown>(null);

const launchOpen = ref(false);
const launchTarget = ref<WorkflowSummary | null>(null);

/** Sandboxes proposées au lancement : toutes, LaunchDialog ne garde que les `Running`. */
const launchSandboxes = computed(() =>
  projectSandboxes.value.map((s) => ({
    name: s.metadata.name,
    phase: s.status?.phase ?? undefined,
  })),
);

/** Libellé dédié du 503 moteur (dashboards.sdd : pas d'erreur brute du serveur). */
const ENGINE_UNAVAILABLE_LABEL = 'Moteur de workflow indisponible';

function describeActionError(err: unknown): string {
  if (err instanceof EngineUnavailable) return ENGINE_UNAVAILABLE_LABEL;
  return err instanceof ApiError ? err.message : String(err);
}

function onCreateWorkflow() {
  editingWorkflow.value = null;
  editorError.value = null;
  editorOpen.value = true;
}

async function onEditWorkflow(id: number) {
  actionError.value = null;
  try {
    editingWorkflow.value = await wfClient.getWorkflow(props.projectName, id);
    editorError.value = null;
    editorOpen.value = true;
  } catch (err) {
    actionError.value = describeActionError(err);
  }
}

async function onSaveWorkflow(body: WorkflowCreate | WorkflowUpdate) {
  editorSaving.value = true;
  editorError.value = null;
  try {
    if (editingWorkflow.value) {
      await wfClient.updateWorkflow(props.projectName, editingWorkflow.value.id, body as WorkflowUpdate);
    } else {
      await wfClient.createWorkflow(props.projectName, body as WorkflowCreate);
    }
    editorOpen.value = false;
    sectionsVersion.value += 1;
  } catch (err) {
    editorError.value = err instanceof EngineUnavailable ? ENGINE_UNAVAILABLE_LABEL : err;
  } finally {
    editorSaving.value = false;
  }
}

function onLaunch(workflow: WorkflowSummary) {
  launchTarget.value = workflow;
  launchOpen.value = true;
}

function onLaunched(_run: RunLaunch) {
  sectionsVersion.value += 1;
}

function openRun(runId: string) {
  router.push(`/p/${encodeURIComponent(props.projectName)}/runs/${encodeURIComponent(runId)}`);
}

// --- Assistant de projet (ContextChat, contexte `project`) -------------------

const assistantOpen = ref(false);
const chatData = computed(() => ({ project_name: props.projectName }));

const formName = ref('');
const formBranch = ref('');
const creationError = ref<string | null>(null);

const modalOpen = ref(false);

async function createSandbox() {
  if (!projectReady.value) return;
  creationError.value = null;
  const body: CreateSandboxBody = {
    name: formName.value,
    project: props.projectName,
    branch: formBranch.value.trim() || 'main',
  };
  try {
    await resource.create(body);
    formName.value = '';
    formBranch.value = '';
    modalOpen.value = false;
  } catch (e) {
    creationError.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function suspendSandbox(name: string) {
  const sandbox = fetchedSandboxes.value.find((s) => s.metadata.name === name);
  if (!sandbox) return;
  const payload = { suspended: !sandbox.spec.suspended };
  try {
    await client.post<Sandbox>(`/api/sandboxes/${encodeURIComponent(name)}/suspend`, payload);
    await resource.fetch();
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : String(e);
  }
}

async function deleteSandbox(name: string) {
  await resource.remove(name);
}
</script>

<template>
  <div class="dashboard" :class="{ 'dashboard--assistant': assistantOpen && !stacked }">
    <div class="dashboard__main">
      <h1>Sandboxes de {{ projectName }}</h1>
      <div class="actions-row">
        <button
          class="btn btn-create"
          :disabled="!projectReady"
          :title="projectReady ? undefined : 'Le clone initial du projet n\'est pas encore terminé'"
          @click="modalOpen = true"
        >
          Créer une sandbox
        </button>
        <button class="btn btn-back" @click="router.push('/')">Retour</button>
        <button class="btn btn-settings" @click="router.push('/settings')">Paramètres</button>
        <button class="btn btn-assistant" @click="assistantOpen = !assistantOpen">Assistant</button>
      </div>
      <p v-if="project && !projectReady" class="not-ready-hint">
        Clone du projet en cours — la création de sandbox sera disponible une fois terminé.
      </p>

      <div class="dash-sections">
        <section class="dash-section" data-section="sandboxes">
          <LoadingSkeleton v-if="loading" />
          <div v-else>
            <ErrorCard v-if="error" :message="error" />
            <div v-else>
              <p v-if="!ideAvailable" class="ide-hint">
                L'IDE est réservé aux écrans d'au moins 1024 px. La vue d'ensemble reste disponible.
              </p>
              <EmptyState v-if="projectSandboxes.length === 0" message="Aucune sandbox." />
              <table class="table" v-else>
                <thead>
                  <tr>
                    <th class="th-name">Nom</th>
                    <th class="th-branch">Branche</th>
                    <th class="th-phase">Phase</th>
                    <th class="th-toolchains">Toolchains</th>
                    <th class="th-action"></th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="s in projectSandboxes" :key="s.metadata.name"
                      @click="onRowClick(s)"
                      :class="{ 'row-clickable': isRunning(s) }">
                    <td>{{ s.metadata.name }}</td>
                    <td>{{ s.spec.branch }}</td>
                    <td>{{ s.status?.phase ?? '—' }}</td>
                    <td>
                      {{
                        s.spec.toolchains && s.spec.toolchains.length > 0
                          ? s.spec.toolchains.map((t) => t.name).join(', ')
                          : '—'
                      }}
                    </td>
                    <td class="td-actions">
                      <button
                        class="btn btn-open"
                        :disabled="!isRunning(s) || !ideAvailable"
                        :title="ideTitle(s)"
                        @click.stop="openIde(s)"
                      >
                        IDE
                      </button>
                      <button
                        class="btn btn-overview"
                        @click.stop="openOverview(s)"
                      >
                        Vue d'ensemble
                      </button>
                      <button
                        class="btn btn-suspend"
                        :class="{ 'btn-suspended': s.spec.suspended }"
                        @click.stop="suspendSandbox(s.metadata.name)"
                      >
                        {{ s.spec.suspended ? 'Reprendre' : 'Suspendre' }}
                      </button>
                      <button class="btn btn-delete" @click.stop="deleteSandbox(s.metadata.name)">
                        Supprimer
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </section>

        <section class="dash-section" data-section="workflows">
          <WorkflowList
            :key="sectionsVersion"
            :project="projectName"
            @create="onCreateWorkflow"
            @edit="onEditWorkflow"
            @launch="onLaunch"
          />
        </section>

        <section class="dash-section" data-section="runs">
          <RunList :key="sectionsVersion" :project="projectName" @select="openRun" />
        </section>
      </div>

      <div v-if="actionError" class="creation-error">{{ actionError }}</div>
    </div>

    <aside
      v-if="assistantOpen"
      class="assistant"
      :data-layout="stacked ? 'sheet' : 'panel'"
      aria-label="Assistant de projet"
    >
      <div class="assistant__bar">
        <button class="btn btn-cancel" @click="assistantOpen = false">Fermer l'assistant</button>
      </div>
      <ContextChat kind="project" :data="chatData" />
    </aside>

    <DialogShell v-model:open="modalOpen" title="Créer une sandbox">
      <Field label="Nom">
        <input
          class="field-input"
          v-model="formName"
          type="text"
          placeholder="ma-sandbox"
          aria-label="Nom de la sandbox"
        />
      </Field>
      <Field label="Branche">
        <input
          class="field-input"
          v-model="formBranch"
          type="text"
          placeholder="main"
          aria-label="Branche"
        />
      </Field>
      <div v-if="creationError" class="creation-error">{{ creationError }}</div>
      <template #actions>
        <button class="btn btn-create" :disabled="!projectReady" @click="createSandbox">
          Créer
        </button>
        <DialogClose class="btn btn-cancel">Annuler</DialogClose>
      </template>
    </DialogShell>

    <DialogShell
      v-model:open="editorOpen"
      :title="editingWorkflow ? 'Éditer le workflow' : 'Nouveau workflow'"
    >
      <WorkflowEditor
        :workflow="editingWorkflow"
        :saving="editorSaving"
        :error="editorError"
        @save="onSaveWorkflow"
        @cancel="editorOpen = false"
      />
    </DialogShell>

    <LaunchDialog
      v-if="launchTarget"
      v-model:open="launchOpen"
      :project="projectName"
      :workflow="launchTarget"
      :scope="launchTarget.scope"
      :sandboxes="launchSandboxes"
      @launched="onLaunched"
    />
  </div>
</template>

<style scoped>
.dashboard {
  background: #0c1420;
  color: #e6e9f0;
  padding: 48px 56px;
}

h1 {
  font-size: 18px;
  font-weight: 600;
  margin: 0 0 16px 0;
}

.actions-row {
  display: flex;
  gap: 12px;
  margin-bottom: 24px;
}

.table {
  width: 100%;
  max-width: 760px;
  border-collapse: collapse;
  margin-bottom: 24px;
}

.row-clickable {
  cursor: pointer;
}

.th-name,
.th-branch,
.th-phase,
.th-toolchains,
.th-action {
  text-align: left;
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-weight: 600;
  font-size: 12px;
  text-transform: uppercase;
  color: #6a7185;
}
.th-name { width: 20%; }
.th-branch { width: 15%; }
.th-phase { width: 15%; }
.th-toolchains { width: 20%; }
.th-action { text-align: right; width: 15%; }

.table td {
  padding: 8px 12px;
  border-bottom: 1px solid #1c1c2a;
  font-size: 13px;
}

.btn-suspend {
  background: #e0a83d22;
  color: #e0a83d;
  border: 1px solid #e0a83d44;
  margin-right: 6px;
}

.btn-suspend:hover {
  background: #e0a83d33;
}

.btn-suspended {
  background: #3fb56d22;
  color: #3fb56d;
  border: 1px solid #3fb56d44;
}

.btn-suspended:hover {
  background: #3fb56d33;
}

.btn-open {
  background: #4c90f0;
  color: white;
  margin-right: 6px;
}

.btn-open:hover {
  background: #3a7de0;
}

.btn-open:disabled {
  background: #2b3550;
  color: #6a7185;
  cursor: not-allowed;
}

.btn-open:disabled:hover {
  background: #2b3550;
}

.btn-back {
  background: #1c1c2a;
  color: #e6e9f0;
  border: 1px solid #2b2b4a;
}

.btn-back:hover {
  background: #2b2b4a;
}

.btn-settings {
  background: #1c1c2a;
  color: #e6e9f0;
  border: 1px solid #2b2b4a;
}

.btn-settings:hover {
  background: #2b2b4a;
}

.btn-create:disabled {
  background: #2b3550;
  color: #6a7185;
  cursor: not-allowed;
}

.btn-create:disabled:hover {
  background: #2b3550;
}

.not-ready-hint {
  color: #6a7185;
  font-size: 13px;
  margin: -12px 0 20px;
}

.btn-assistant {
  background: #4c90f0;
  color: white;
}

.btn-overview {
  background: #1c1c2a;
  color: #e6e9f0;
  border: 1px solid #2b2b4a;
  margin-right: 6px;
}

.btn-overview:hover {
  background: #2b2b4a;
}

.td-actions {
  text-align: right;
}

.ide-hint {
  color: #6a7185;
  font-size: 13px;
  margin: 0 0 12px;
}

/* Sections : une colonne sous md, grille à partir de md (sandboxes pleine largeur). */
.dash-sections {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 24px;
  align-items: start;
}

.dash-section {
  min-width: 0;
}

.dashboard--assistant {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(320px, 400px);
  gap: 24px;
  align-items: start;
}

.dashboard__main {
  min-width: 0;
}

.assistant {
  display: flex;
  flex-direction: column;
  min-width: 0;
  height: 70vh;
  border: 1px solid #2b2b4a;
  border-radius: 8px;
  overflow: hidden;
  background: #0c1420;
}

.assistant__bar {
  display: flex;
  justify-content: flex-end;
  padding: 8px 12px;
}

@media (min-width: 768px) {
  .dash-sections {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .dash-section[data-section='sandboxes'] {
    grid-column: 1 / -1;
  }
}

/* Sous md : feuille plein écran, marges de 16 px, tableaux empilés (cartes). */
@media (max-width: 767px) {
  .dashboard {
    padding: 24px 16px;
  }
  .assistant {
    position: fixed;
    inset: 0;
    z-index: 900;
    height: auto;
    border: 0;
    border-radius: 0;
  }
  .table,
  .table tbody,
  .table tr,
  .table td {
    display: block;
    width: 100%;
  }
  .table thead {
    display: none;
  }
  .table tr {
    border-bottom: 1px solid #1c1c2a;
    padding: 8px 0;
  }
  .td-actions {
    text-align: left;
  }
}
</style>
