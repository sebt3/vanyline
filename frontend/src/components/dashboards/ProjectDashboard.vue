<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue';
import { useRouter } from 'vue-router';
import { DialogClose } from 'reka-ui';
import type { RunLaunch, Workflow, WorkflowCreate, WorkflowSummary, WorkflowUpdate } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import { useCrudResource } from '../../composables/useCrudResource';
import {
  useSandboxState,
  registerRefresh,
  unregisterRefresh,
  registerProjectRefresh,
  unregisterProjectRefresh,
} from '../../composables/useSandboxState';
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
  spec?: { repoUrl?: string; defaultBranch?: string | null } | null;
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
  // Événement `project` (clone terminé) : relire le Project, sans recharger la page.
  registerProjectRefresh(fetchProject);
});

onBeforeUnmount(() => {
  unregisterRefresh(resource.fetch);
  unregisterProjectRefresh(fetchProject);
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

/** Ton de la pastille de phase : libellé texte toujours présent, couleur d'état seulement. */
function phaseTone(sandbox: Sandbox): 'ok' | 'warn' | 'err' | 'muted' {
  const phase = sandbox.status?.phase;
  if (!phase) return 'muted';
  if (phase === 'Running') return 'ok';
  if (phase === 'Suspended') return 'muted';
  if (/fail|error/i.test(phase)) return 'err';
  return 'warn';
}

function toolchainLabel(sandbox: Sandbox): string {
  const names = sandbox.spec.toolchains?.map((t) => t.name) ?? [];
  return names.length > 0 ? names.join(', ') : '—';
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
  <div class="dashboard">
    <div class="dashboard__main">
      <header class="page-head">
        <div class="page-title">
          <h1>{{ projectName }}</h1>
          <p v-if="project?.spec?.repoUrl" class="page-meta">
            <span class="mono">{{ project.spec.repoUrl }}</span>
            <span v-if="project.spec.defaultBranch" class="mono"> · {{ project.spec.defaultBranch }}</span>
          </p>
          <p v-if="project" class="page-state">
            <span class="dot" :class="projectReady ? 'dot--ok' : 'dot--warn'" aria-hidden="true"></span>
            {{ projectReady ? 'Cloné' : 'Clonage en cours' }}
          </p>
        </div>
        <div class="page-actions">
          <button
            class="btn btn-create"
            :disabled="!projectReady"
            :title="projectReady ? undefined : 'Le clone initial du projet n\'est pas encore terminé'"
            @click="modalOpen = true"
          >
            Nouvelle sandbox
          </button>
          <button class="btn btn-back" @click="router.push('/')">Retour</button>
          <button
            class="btn btn-assistant"
            :aria-expanded="assistantOpen"
            @click="assistantOpen = !assistantOpen"
          >
            Assistant
          </button>
        </div>
      </header>
      <p v-if="project && !projectReady" class="not-ready-hint">
        Clone du projet en cours — la création de sandbox sera disponible une fois terminé.
      </p>

      <div class="dash-sections">
        <section class="dash-section" data-section="sandboxes">
          <h2 class="section-title">Sandboxes</h2>
          <LoadingSkeleton v-if="loading" />
          <div v-else>
            <ErrorCard v-if="error" :message="error" />
            <div v-else>
              <p v-if="!ideAvailable" class="ide-hint">
                L'IDE est réservé aux écrans d'au moins 1024 px. La vue d'ensemble reste disponible.
              </p>
              <EmptyState v-if="projectSandboxes.length === 0" message="Aucune sandbox." />
              <ul v-else class="sandbox-grid">
                <li v-for="s in projectSandboxes" :key="s.metadata.name" class="sandbox-item">
                  <article
                    class="sandbox-card"
                    :class="{ 'row-clickable': isRunning(s) }"
                    @click="onRowClick(s)"
                  >
                    <div class="sandbox-card__head">
                      <span class="sandbox-card__name">{{ s.metadata.name }}</span>
                      <details class="card-menu" @click.stop>
                        <summary class="card-menu__toggle" :aria-label="`Actions de la sandbox ${s.metadata.name}`">…</summary>
                        <div class="card-menu__panel">
                          <button class="btn btn-delete" @click.stop="deleteSandbox(s.metadata.name)">
                            Supprimer
                          </button>
                        </div>
                      </details>
                    </div>
                    <p class="sandbox-card__branch mono">{{ s.spec.branch }}</p>
                    <p class="sandbox-card__phase">
                      <span class="dot" :class="`dot--${phaseTone(s)}`" aria-hidden="true"></span>
                      {{ s.status?.phase ?? '—' }}
                    </p>
                    <div class="pills">
                      <span v-for="t in (s.spec.toolchains ?? [])" :key="t.name" class="pill">{{ t.name }}</span>
                      <span v-if="toolchainLabel(s) === '—'" class="pill pill--muted">aucune toolchain</span>
                    </div>
                    <div class="sandbox-card__actions">
                      <button
                        class="btn btn-open"
                        :disabled="!isRunning(s) || !ideAvailable"
                        :title="ideTitle(s)"
                        @click.stop="openIde(s)"
                      >
                        IDE
                      </button>
                      <button class="btn btn-overview" @click.stop="openOverview(s)">
                        Vue d'ensemble
                      </button>
                      <button
                        class="btn btn-suspend"
                        :class="{ 'btn-suspended': s.spec.suspended }"
                        @click.stop="suspendSandbox(s.metadata.name)"
                      >
                        {{ s.spec.suspended ? 'Reprendre' : 'Suspendre' }}
                      </button>
                    </div>
                  </article>
                </li>
              </ul>
            </div>
          </div>
        </section>

        <div class="dash-split">
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
      </div>

      <div v-if="actionError" class="creation-error">{{ actionError }}</div>

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

    <AssistantPanel
      :open="assistantOpen"
      title="Assistant de projet"
      :data-layout="stacked ? 'sheet' : 'panel'"
      @close="assistantOpen = false"
    >
      <ContextChat kind="project" :data="chatData" />
    </AssistantPanel>
  </div>
</template>

<style scoped>
/* Coquille de l'écran : la page ne défile pas, la région de contenu défile en interne et
   l'assistant (AssistantPanel) occupe la colonne de droite à partir de md (layout.sdd). */
.dashboard {
  display: flex;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--bg);
  color: var(--text);
  font-family: var(--font-sans);
}

.dashboard__main {
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  padding: 24px 28px;
}

.mono {
  font-family: var(--font-mono);
}

.page-head {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 20px;
}

h1 {
  font-size: 18px;
  font-weight: 600;
  margin: 0 0 4px;
  overflow-wrap: anywhere;
}

.page-meta {
  margin: 0;
  font-size: 12px;
  color: var(--muted);
  overflow-wrap: anywhere;
}

.page-state {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--muted);
}

.page-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.page-actions .btn,
.sandbox-card__actions .btn,
.card-menu .btn-delete {
  font: inherit;
  cursor: pointer;
}

.page-actions .btn {
  display: inline-flex;
  align-items: center;
  min-height: var(--ctl-h);
  padding: 0 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  font-weight: 500;
}

.page-actions .btn-create {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}

.page-actions .btn-create:disabled {
  background: var(--surface-2);
  border-color: var(--border);
  color: var(--muted);
  cursor: not-allowed;
}

.page-actions .btn-back,
.page-actions .btn-assistant {
  background: var(--surface-2);
  color: var(--text);
}

.page-actions .btn:focus-visible,
.sandbox-card:focus-visible,
.card-menu__toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.not-ready-hint {
  color: var(--muted);
  font-size: 13px;
  margin: -8px 0 16px;
}

.ide-hint {
  color: var(--muted);
  font-size: 13px;
  margin: 0 0 12px;
}

.section-title {
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--muted);
  margin: 0 0 10px;
}

/* Sandboxes en cartes ; pleine largeur. */
.sandbox-grid {
  list-style: none;
  margin: 0 0 24px;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 16px;
}

.sandbox-item {
  min-width: 0;
}

.sandbox-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-width: 0;
  padding: 14px 16px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
}

.sandbox-card.row-clickable {
  cursor: pointer;
}

.sandbox-card.row-clickable:hover {
  border-color: var(--accent);
}

.sandbox-card__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

.sandbox-card__name {
  font-weight: 600;
  overflow-wrap: anywhere;
}

.sandbox-card__branch {
  margin: 0;
  font-size: 12px;
  color: var(--muted);
  overflow-wrap: anywhere;
}

.sandbox-card__phase {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 12px;
  color: var(--text);
}

.sandbox-card__actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: auto;
}

.sandbox-card__actions .btn {
  display: inline-flex;
  align-items: center;
  min-height: var(--ctl-h);
  padding: 0 10px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--surface-2);
  color: var(--text);
}

.sandbox-card__actions .btn:hover {
  background: var(--surface);
}

.sandbox-card__actions .btn-open {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--bg);
}

.sandbox-card__actions .btn-open:hover {
  filter: brightness(1.08);
}

.sandbox-card__actions .btn-open:disabled {
  background: var(--surface-2);
  border-color: var(--border);
  color: var(--muted);
  cursor: not-allowed;
}

.sandbox-card__actions .btn-suspend {
  color: var(--warn);
}

.sandbox-card__actions .btn-suspended {
  color: var(--ok);
}

.dot {
  display: inline-block;
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--muted);
}
.dot--ok {
  background: var(--ok);
}
.dot--warn {
  background: var(--warn);
}
.dot--err {
  background: var(--err);
}

.pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.pill {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--surface-2);
  color: var(--muted);
  font-size: 11px;
  white-space: nowrap;
}

.pill--muted {
  font-style: italic;
}

/* Menu de la carte : suppression repliée, jamais un bouton rouge permanent. */
.card-menu {
  position: relative;
  z-index: 1;
}

.card-menu__toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--ctl-h);
  height: var(--ctl-h);
  border-radius: 6px;
  color: var(--muted);
  cursor: pointer;
  list-style: none;
  user-select: none;
}

.card-menu__toggle::-webkit-details-marker {
  display: none;
}

.card-menu__toggle:hover {
  color: var(--text);
  background: var(--surface-2);
}

.card-menu__panel {
  position: absolute;
  right: 0;
  top: calc(var(--ctl-h) + 4px);
  z-index: 2;
  padding: 4px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.card-menu .btn-delete {
  display: block;
  min-height: var(--ctl-h);
  padding: 0 12px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--err);
  white-space: nowrap;
}

.card-menu .btn-delete:hover {
  background: var(--surface);
}

/* Sections : une colonne ; à partir de md, WORKFLOWS et RUNS côte à côte (auto-fit 340 px). */
.dash-sections {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.dash-split {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
  gap: 24px;
  align-items: start;
}

.dash-section {
  min-width: 0;
}

/* Sous md : marges de 16 px. */
@media (max-width: 767px) {
  .dashboard__main {
    padding: 16px;
  }
}
</style>
