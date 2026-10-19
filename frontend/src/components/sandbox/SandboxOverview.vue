<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { RouterLink, useRouter } from 'vue-router';
import type { HealthReport, HealthState, RunLaunch, WorkflowSummary } from '@vanyline/protocol';
import { ApiError, createApiClient } from '../../api/client';
import { createWorkflowsClient, EngineUnavailable } from '../../api/workflowsClient';
import type { WorkflowsClient } from '../../api/workflowsClient';
import { useSandboxState } from '../../composables/useSandboxState';
import ContextChat from '../ContextChat.vue';
import ErrorCard from '../common/ErrorCard.vue';
import LoadingSkeleton from '../common/LoadingSkeleton.vue';
import { LG_MIN_WIDTH } from '../dashboards/viewport';
import LaunchDialog from '../workflows/LaunchDialog.vue';
import RunList from '../workflows/RunList.vue';

/** Intervalles d'interrogation de la santé du code (sandbox-overview.sdd). */
const POLL_BUSY_MS = 10_000;
const POLL_IDLE_MS = 30_000;

type Check = HealthReport['checks'][number];

interface Sandbox {
  metadata: { name: string };
  spec: { project: string; branch: string; suspended?: boolean };
  status?: { phase?: string | null };
}

const props = defineProps<{ projectName: string; sandboxName: string }>();

const router = useRouter();
const api = createApiClient();
const workflows: WorkflowsClient = createWorkflowsClient(api);
const { phases } = useSandboxState();

const segment = (s: string) => encodeURIComponent(s);

function describeError(err: unknown): string {
  if (err instanceof EngineUnavailable) return 'Moteur de workflow indisponible.';
  return err instanceof ApiError ? err.message : String(err);
}

// --- En-tête : sandbox, phase, suspension ----------------------------------

const sandbox = ref<Sandbox | null>(null);
const sandboxError = ref<string | null>(null);
const suspendError = ref<string | null>(null);

/** Phase K8s : le WebSocket d'états prime sur la valeur lue au chargement. */
const phase = computed<string | null>(
  () => phases.value.get(props.sandboxName) ?? sandbox.value?.status?.phase ?? null,
);
const isRunning = computed(() => phase.value === 'Running');

async function loadSandbox() {
  try {
    sandbox.value = await api.get<Sandbox>(`/api/sandboxes/${segment(props.sandboxName)}`);
    sandboxError.value = null;
  } catch (err) {
    sandboxError.value = describeError(err);
  }
}

async function toggleSuspend() {
  if (!sandbox.value) return;
  suspendError.value = null;
  try {
    await api.post<unknown>(`/api/sandboxes/${segment(props.sandboxName)}/suspend`, {
      suspended: !sandbox.value.spec.suspended,
    });
    await loadSandbox();
  } catch (err) {
    suspendError.value = describeError(err);
  }
}

// --- Largeur : lien vers l'IDE --------------------------------------------

const wideEnough = ref(window.innerWidth >= LG_MIN_WIDTH);
function onResize() {
  wideEnough.value = window.innerWidth >= LG_MIN_WIDTH;
}

// --- Carte SANTÉ ------------------------------------------------------------

const health = ref<HealthState | null>(null);
const healthError = ref<string | null>(null);
const refreshError = ref<string | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined;

const report = computed<HealthReport | null>(() => health.value?.report ?? null);

const freshness = computed<{ label: string; tone: string } | null>(() => {
  // Sondage en échec : le dernier rapport n'est plus confirmé, il ne peut pas être « à jour ».
  if (healthError.value && health.value) return { label: 'Possiblement périmé', tone: 'warn' };
  switch (health.value?.phase) {
    case 'computing':
      return { label: 'Calcul en cours', tone: 'busy' };
    case 'stale':
      return { label: 'Périmé', tone: 'warn' };
    case 'fresh':
      return { label: 'À jour', tone: 'ok' };
    default:
      return null;
  }
});

const checkLabels: Record<Check['status'], string> = {
  passed: 'Réussie',
  failed: 'Échec',
  errored: 'Erreur',
  skipped: 'Ignorée',
};

const checkSummary = computed(() => {
  const checks = report.value?.checks ?? [];
  const count = (status: Check['status']) => checks.filter((c) => c.status === status).length;
  return {
    passed: count('passed'),
    failed: count('failed'),
    errored: count('errored'),
    skipped: count('skipped'),
  };
});

const erroredChecks = computed<Check[]>(
  () => report.value?.checks.filter((c) => c.status === 'errored') ?? [],
);

/** Statut global et pourcentages : lus tels quels depuis `summary` (dérivé par la sandbox). */
const summary = computed(() => health.value?.summary ?? null);

const verdictLabels: Record<Check['status'], string> = {
  passed: 'Réussi',
  failed: 'Échec',
  errored: 'Erreur de mesure',
  skipped: 'Ignoré',
};

const verdictTone = computed(() => {
  switch (summary.value?.status) {
    case 'passed':
      return 'ok';
    case 'failed':
      return 'fail';
    case undefined:
      return '';
    default:
      return 'warn';
  }
});

/** Lacunes de couverture par chemin, pour l'affichage (pas de pourcentage recalculé). */
const gapsByPath = computed(
  () => new Map((report.value?.coverage?.files ?? []).map((f) => [f.path, f.uncovered])),
);

/** Format d'affichage d'un pourcentage déjà calculé par la sandbox. */
function formatPercent(value: number): string {
  return Number.isInteger(value) ? `${value}` : value.toFixed(1);
}

const issuesBySeverity = computed(() => {
  const issues = report.value?.issues ?? [];
  return {
    error: issues.filter((i) => i.severity === 'error'),
    warning: issues.filter((i) => i.severity === 'warning'),
  };
});

/** Chemin tronqué au milieu, nom de fichier toujours visible. */
function truncatePath(path: string, max = 40): string {
  if (path.length <= max) return path;
  const slash = path.lastIndexOf('/');
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  if (name.length >= max - 2) return `…${name.slice(-(max - 1))}`;
  const keep = max - name.length - 2;
  return `${path.slice(0, keep)}…/${name}`;
}

function gapText(ranges: { start: number; end: number }[]): string {
  const shown = ranges.slice(0, 3).map((r) => `${r.start}–${r.end}`).join(', ');
  return ranges.length > 3 ? `${shown}…` : shown;
}

const severities = ['error', 'warning'] as const;

function formatDate(ms: number | null | undefined): string {
  return ms == null ? '—' : new Date(ms).toLocaleString('fr-FR');
}

function pollDelay(): number {
  return health.value === null || health.value.phase !== 'fresh' ? POLL_BUSY_MS : POLL_IDLE_MS;
}

function stopPolling() {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
}

function schedulePoll() {
  stopPolling();
  if (document.visibilityState === 'hidden') return;
  timer = setTimeout(() => void loadHealth(), pollDelay());
}

async function loadHealth() {
  stopPolling();
  try {
    health.value = await workflows.getCodeHealth(props.sandboxName);
    healthError.value = null;
  } catch (err) {
    healthError.value = describeError(err);
  } finally {
    schedulePoll();
  }
}

async function refreshHealth() {
  refreshError.value = null;
  try {
    await workflows.refreshCodeHealth(props.sandboxName);
    await loadHealth();
  } catch (err) {
    refreshError.value = describeError(err);
  }
}

function onVisibilityChange() {
  if (document.visibilityState === 'hidden') {
    stopPolling();
  } else {
    void loadHealth();
  }
}

// --- Carte WORKFLOWS (portée sandbox, lancement sur cette sandbox) ----------

const sandboxWorkflows = ref<WorkflowSummary[]>([]);
const workflowsError = ref<string | null>(null);
const launchTarget = ref<WorkflowSummary | null>(null);
const launchOpen = ref(false);
/** Incrémenté après un lancement ou un tour de chat : force le rechargement de la carte runs. */
const runsKey = ref(0);

async function loadWorkflows() {
  try {
    const all = await workflows.listWorkflows(props.projectName);
    sandboxWorkflows.value = all.filter((w) => w.scope === 'sandbox');
    workflowsError.value = null;
  } catch (err) {
    workflowsError.value = describeError(err);
  }
}

const sandboxChoices = computed(() => [
  { name: props.sandboxName, phase: phase.value ?? undefined },
]);

function askLaunch(wf: WorkflowSummary) {
  launchTarget.value = wf;
  launchOpen.value = true;
}

function onLaunched(_run: RunLaunch) {
  runsKey.value += 1;
}

// --- Carte RUNS -------------------------------------------------------------

function openRun(runId: string) {
  void router.push({ name: 'run', params: { runId } });
}

function onTurnFinished() {
  runsKey.value += 1;
  void loadWorkflows();
}

// --- Chat (feuille plein écran sous md) ------------------------------------

const chatOpen = ref(false);

// --- Cycle de vie -----------------------------------------------------------

onMounted(() => {
  void loadSandbox();
  void loadHealth();
  void loadWorkflows();
  window.addEventListener('resize', onResize);
  document.addEventListener('visibilitychange', onVisibilityChange);
});

onBeforeUnmount(() => {
  stopPolling();
  window.removeEventListener('resize', onResize);
  document.removeEventListener('visibilitychange', onVisibilityChange);
});
</script>

<template>
  <div class="overview">
    <header class="card header">
      <div class="header-main">
        <h1>{{ sandboxName }}</h1>
        <p class="meta">
          Projet
          <RouterLink :to="`/p/${projectName}`">{{ projectName }}</RouterLink>
          · branche <span data-testid="branch">{{ sandbox?.spec.branch ?? '—' }}</span>
        </p>
        <p class="meta">
          Phase : <strong data-testid="phase">{{ phase ?? '—' }}</strong>
        </p>
      </div>
      <div class="header-actions">
        <button
          type="button"
          class="btn"
          data-testid="suspend"
          :disabled="!sandbox"
          @click="toggleSuspend"
        >
          {{ sandbox?.spec.suspended ? 'Reprendre' : 'Suspendre' }}
        </button>
        <RouterLink
          v-if="wideEnough"
          data-testid="ide-link"
          class="btn"
          :to="`/p/${projectName}/s/${sandboxName}`"
        >
          Ouvrir l'IDE
        </RouterLink>
        <div v-else data-testid="ide-disabled" class="ide-unavailable">
          <button type="button" class="btn" disabled>Ouvrir l'IDE</button>
          <p>L'IDE nécessite une fenêtre d'au moins 1024 px de large.</p>
        </div>
      </div>
      <ErrorCard v-if="sandboxError" :message="sandboxError" />
      <ErrorCard v-if="suspendError" :message="suspendError" />
    </header>

    <div class="grid">
      <section class="card" aria-labelledby="health-title">
        <div class="card-head">
          <h2 id="health-title">Santé du code</h2>
          <span
            v-if="freshness"
            data-testid="freshness"
            class="badge"
            :class="`badge--${freshness.tone}`"
          >
            {{ freshness.label }}
          </span>
        </div>

        <div v-if="healthError && health" data-testid="poll-error" class="warning" role="alert">
          Actualisation impossible : données possiblement périmées ({{ healthError }}).
        </div>
        <ErrorCard v-if="healthError && !health" :message="healthError" />
        <div v-else-if="!summary || !report" data-testid="health-loading" class="loading" role="status">
          <LoadingSkeleton />
          <p>Premier calcul en cours…</p>
        </div>

        <template v-else>
          <p data-testid="verdict" class="verdict" :class="`verdict--${verdictTone}`">
            Statut global : <strong>{{ summary.status }}</strong> — {{ verdictLabels[summary.status] }}
          </p>

          <p v-if="health?.computed_at != null" data-testid="computed-at" class="meta">
            Calculé le {{ formatDate(health.computed_at) }}
            <template v-if="health.phase === 'stale' && health.stale_since != null">
              · modifié depuis le {{ formatDate(health.stale_since) }}
            </template>
          </p>

          <div v-if="erroredChecks.length" data-testid="errored-warning" class="warning" role="alert">
            Vérification en erreur : {{ erroredChecks.map((c) => c.id).join(', ') }}.
            Le résultat n'est pas une mesure complète.
          </div>

          <ul class="checks">
            <li
              v-for="check in report.checks"
              :key="check.id"
              data-testid="check-row"
              class="check"
              :class="`check--${check.status}`"
            >
              <span class="check-id">{{ check.id }}</span>
              <span class="check-kind">{{ check.kind }}</span>
              <span class="check-status">{{ checkLabels[check.status] }}</span>
            </li>
          </ul>
          <p data-testid="checks-summary" class="meta">
            {{ checkSummary.passed }} réussie(s), {{ checkSummary.failed }} échec(s),
            {{ checkSummary.errored }} erreur(s), {{ checkSummary.skipped }} ignorée(s)
          </p>

          <div class="coverage">
            <p v-if="summary.coverage_percent !== null" data-testid="coverage-total">
              Couverture : <strong>{{ formatPercent(summary.coverage_percent) }} %</strong>
            </p>
            <p v-else data-testid="coverage-missing" class="meta">
              Pas de mesure de couverture.
            </p>
            <ul v-if="summary.files.length" class="files">
              <li v-for="f in summary.files" :key="f.path" data-testid="file-coverage">
                <span class="file-path" :title="f.path">{{ truncatePath(f.path) }}</span>
                <span class="file-pct">{{ formatPercent(f.percent) }} %</span>
                <span v-if="(gapsByPath.get(f.path) ?? []).length" class="file-gaps">
                  non couvert : {{ gapText(gapsByPath.get(f.path) ?? []) }}
                </span>
              </li>
            </ul>
          </div>

          <div v-for="sev in severities" :key="sev">
            <details v-if="issuesBySeverity[sev].length" data-testid="issues-group" class="issues">
              <summary>
                {{ sev === 'error' ? 'Erreurs' : 'Avertissements' }} ({{ issuesBySeverity[sev].length }})
              </summary>
              <ul>
                <li v-for="(issue, i) in issuesBySeverity[sev]" :key="i" class="issue">
                  <span data-testid="issue-path" class="issue-path" :title="issue.path">
                    {{ truncatePath(issue.path) }}:{{ issue.line }}
                  </span>
                  <span class="issue-msg">{{ issue.message }}</span>
                  <span class="issue-check">{{ issue.check_id }}</span>
                </li>
              </ul>
            </details>
          </div>

          <div v-if="report.unchecked_languages.length" class="warnings" role="status">
            <p
              v-for="lang in report.unchecked_languages"
              :key="lang"
              data-testid="unchecked-warning"
              class="warning"
            >
              {{ lang }} non vérifié
            </p>
          </div>

          <div v-if="report.new_code" data-testid="new-code" class="new-code">
            <p>Code neuf (base {{ report.new_code.base }}) : {{ report.new_code.issues.length }} issue(s)</p>
          </div>
        </template>

        <div class="actions">
          <button type="button" class="btn" data-testid="refresh" @click="refreshHealth">
            Recalculer
          </button>
        </div>
        <ErrorCard v-if="refreshError" :message="refreshError" />
      </section>

      <section class="card" aria-labelledby="wf-title">
        <h2 id="wf-title">Workflows</h2>
        <ErrorCard v-if="workflowsError" :message="workflowsError" />
        <p v-if="!isRunning" class="warning" data-testid="launch-disabled">
          La sandbox n'est pas <strong>Running</strong> (phase actuelle : {{ phase ?? '—' }}) :
          le lancement est désactivé.
        </p>
        <ul v-if="sandboxWorkflows.length" class="workflows">
          <li v-for="wf in sandboxWorkflows" :key="wf.id" class="workflow">
            <span>{{ wf.name }}</span>
            <button
              type="button"
              class="btn"
              :data-testid="`launch-${wf.id}`"
              :disabled="!isRunning"
              @click="askLaunch(wf)"
            >
              Lancer
            </button>
          </li>
        </ul>
        <p v-else-if="!workflowsError" class="meta">Aucun workflow de portée sandbox.</p>
        <LaunchDialog
          v-if="launchTarget"
          v-model:open="launchOpen"
          :project="projectName"
          :workflow="launchTarget"
          scope="sandbox"
          :fixed-sandbox="sandboxName"
          :sandboxes="sandboxChoices"
          @launched="onLaunched"
        />
      </section>

      <section class="card" aria-labelledby="runs-title">
        <h2 id="runs-title">Runs de cette sandbox</h2>
        <RunList :key="runsKey" :sandbox="sandboxName" @select="openRun" />
      </section>

      <section class="card chat" :class="{ 'chat--open': chatOpen }" aria-labelledby="chat-title">
        <div class="card-head">
          <h2 id="chat-title">Assistant</h2>
          <button type="button" class="btn chat-toggle" @click="chatOpen = !chatOpen">
            {{ chatOpen ? 'Fermer' : 'Ouvrir l\'assistant' }}
          </button>
        </div>
        <ContextChat kind="sandbox" :data="{ sandbox_name: sandboxName }" @turn-finished="onTurnFinished" />
      </section>
    </div>
  </div>
</template>

<style scoped>
.overview {
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 16px;
  max-width: 100%;
  overflow-x: hidden;
}

.grid {
  display: flex;
  flex-direction: column;
  gap: 16px;
  min-width: 0;
}

.card {
  box-sizing: border-box;
  min-width: 0;
  border: 1px solid rgba(127, 127, 127, 0.35);
  border-radius: 8px;
  padding: 16px;
  overflow-wrap: anywhere;
}

.card-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

h1 {
  font-size: 1.4rem;
  margin: 0 0 4px;
}

h2 {
  font-size: 1.1rem;
  margin: 0 0 8px;
}

.header-main {
  min-width: 0;
}

.header-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 12px;
}

.meta {
  font-size: 0.9rem;
  opacity: 0.8;
  margin: 2px 0;
}

.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 44px;
  min-width: 44px;
  padding: 0 14px;
  border: 1px solid rgba(127, 127, 127, 0.5);
  border-radius: 6px;
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: pointer;
  text-decoration: none;
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.badge {
  font-size: 0.8rem;
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid currentColor;
}

.badge--ok { color: #1a7f37; }
.badge--warn { color: #9a6700; }
.badge--busy { color: #0969da; }
.verdict--ok { color: #1a7f37; }
.verdict--warn { color: #9a6700; }
.verdict--fail { color: #cf222e; }

.warning {
  margin: 8px 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: rgba(214, 153, 34, 0.15);
  color: #9a6700;
}

.checks,
.workflows,
.files,
.issues ul {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}

.check,
.workflow {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  padding: 6px 0;
  border-bottom: 1px solid rgba(127, 127, 127, 0.2);
}

.check--errored { color: #9a6700; }
.check--failed { color: #cf222e; }

.check-kind {
  opacity: 0.7;
  font-size: 0.85rem;
}

.file-path,
.issue-path {
  display: inline-block;
  max-width: 100%;
  font-family: ui-monospace, monospace;
  font-size: 0.85rem;
}

.file-pct {
  margin-left: 8px;
  font-weight: 600;
}

.file-gaps,
.issue-check {
  display: block;
  font-size: 0.8rem;
  opacity: 0.75;
}

.issues summary {
  min-height: 44px;
  display: flex;
  align-items: center;
  cursor: pointer;
}

.issue {
  padding: 6px 0;
  border-bottom: 1px solid rgba(127, 127, 127, 0.2);
}

.actions {
  margin-top: 12px;
}

.ide-unavailable p {
  font-size: 0.85rem;
  opacity: 0.8;
}

.chat-toggle {
  display: none;
}

@media (min-width: 768px) {
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    align-items: start;
  }
}

@media (max-width: 767px) {
  .chat {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: none;
    overflow-y: auto;
    border-radius: 0;
    background: var(--color-background, #fff);
  }

  .chat--open {
    display: block;
  }

  .chat-toggle {
    display: inline-flex;
  }
}

@media (min-width: 768px) {
  .chat {
    display: block;
  }
}
</style>
