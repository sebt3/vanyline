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
import { LG_MIN_WIDTH, MD_MIN_WIDTH, useViewportWidth } from '../dashboards/viewport';
import AssistantPanel from '../shell/AssistantPanel.vue';
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

const width = useViewportWidth();
const wideEnough = computed(() => width.value >= LG_MIN_WIDTH);
/** Sous md : une section à la fois, barre d'onglets en bas (sandbox-overview.sdd, Refonte). */
const stacked = computed(() => width.value < MD_MIN_WIDTH);

// --- Onglets (sous md) et assistant ------------------------------------------

type Tab = 'health' | 'workflows' | 'runs' | 'assistant';
const tabs: { id: Tab; label: string }[] = [
  { id: 'health', label: 'Santé' },
  { id: 'workflows', label: 'Workflows' },
  { id: 'runs', label: 'Exécutions' },
  { id: 'assistant', label: 'Assistant' },
];
const activeTab = ref<Tab>('health');
/** Ouverture de l'assistant à partir de md (bouton de l'en-tête). */
const assistantOpen = ref(false);

/** Sous md, l'onglet « Assistant » ouvre la feuille ; à partir de md, le bouton de l'en-tête. */
const panelOpen = computed(() =>
  stacked.value ? activeTab.value === 'assistant' : assistantOpen.value,
);

function sectionVisible(tab: Exclude<Tab, 'assistant'>): boolean {
  return !stacked.value || activeTab.value === tab;
}

function closeAssistant() {
  assistantOpen.value = false;
  if (activeTab.value === 'assistant') activeTab.value = 'health';
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

/**
 * Synthèse de la carte santé : décompte d'affichage des vérifications non passed/skipped
 * (aucune interprétation du rapport).
 */
const toCorrect = computed(
  () => (report.value?.checks ?? []).filter((c) => c.status !== 'passed' && c.status !== 'skipped').length,
);

const KIND_LABELS: Record<Check['kind'], string> = {
  format: 'Format',
  lint: 'Clippy / lint',
  tests: 'Tests',
  coverage: 'Couverture',
  new_code: 'Code neuf',
};

interface Tile {
  id: string;
  name: string;
  state: string;
  tone: string;
  value: string;
}

/** Vignettes : une par vérification, valeur issue des compteurs d'issues du rapport. */
const checkTiles = computed<Tile[]>(() => {
  const issues = report.value?.issues ?? [];
  return (report.value?.checks ?? []).map((c): Tile => {
    const own = issues.filter((i) => i.check_id === c.id);
    const errors = own.filter((i) => i.severity === 'error').length;
    const warnings = own.length - errors;
    const plural = (n: number, word: string) => `${n} ${word}${n > 1 ? 's' : ''}`;
    let value: string;
    if (errors > 0) value = plural(errors, 'erreur');
    else if (warnings > 0) value = plural(warnings, 'avertissement');
    else if (c.status === 'passed' && c.kind !== 'tests') value = 'propre';
    else value = checkLabels[c.status];
    const tone = c.status === 'passed' ? 'ok' : c.status === 'skipped' ? 'muted' : c.status === 'failed' ? 'fail' : 'warn';
    return { id: c.id, name: `${c.id} (${KIND_LABELS[c.kind]})`, state: checkLabels[c.status], tone, value };
  });
});

const coverageTile = computed<Tile>(() => {
  const coverage = summary.value?.coverage_percent;
  return coverage == null
    ? { id: 'coverage', name: 'Couverture', state: 'Non mesurée', tone: 'warn', value: '—' }
    : { id: 'coverage', name: 'Couverture', state: 'Mesurée', tone: 'muted', value: `${formatPercent(coverage)} %` };
});

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

// --- Cycle de vie -----------------------------------------------------------

onMounted(() => {
  void loadSandbox();
  void loadHealth();
  void loadWorkflows();
  document.addEventListener('visibilitychange', onVisibilityChange);
});

onBeforeUnmount(() => {
  stopPolling();
  document.removeEventListener('visibilitychange', onVisibilityChange);
});
</script>

<template>
  <div class="screen">
  <div class="overview" :class="{ 'overview--stacked': stacked }">
    <header class="card header">
      <div class="header-row">
        <RouterLink
          class="back"
          :to="`/p/${projectName}`"
          :aria-label="`Retour au projet ${projectName}`"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
            <path d="M15 6l-6 6 6 6" />
          </svg>
        </RouterLink>
        <div class="header-main">
          <h1>{{ sandboxName }}</h1>
          <p class="meta">
            Projet <span data-testid="project">{{ projectName }}</span>
            · branche <span data-testid="branch">{{ sandbox?.spec.branch ?? '—' }}</span>
          </p>
        </div>
        <span
          data-testid="phase"
          class="phase-pill"
          :class="`phase--${phase === 'Running' ? 'ok' : 'muted'}`"
        >
          {{ phase ?? '—' }}
        </span>
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
        <button
          v-if="!stacked"
          type="button"
          class="btn"
          data-testid="assistant-toggle"
          :aria-expanded="assistantOpen"
          @click="assistantOpen = !assistantOpen"
        >
          Assistant
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
      <section
        v-show="sectionVisible('health')"
        data-testid="section-health"
        class="card"
        aria-labelledby="health-title"
      >
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
          <p
            data-testid="health-synthesis"
            class="synthesis"
            :class="toCorrect > 0 ? 'synthesis--warn' : 'synthesis--ok'"
          >
            {{ toCorrect > 0 ? `${toCorrect} vérification${toCorrect > 1 ? 's' : ''} à corriger` : 'Tout est vert' }}
          </p>
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

          <ul class="tiles">
            <li
              v-for="tile in checkTiles"
              :key="tile.id"
              data-testid="check-row"
              class="tile"
              :class="`tile--${tile.tone}`"
            >
              <div :data-testid="`tile-${tile.id}`" class="tile-body">
                <span class="tile-name">{{ tile.name }}</span>
                <span class="tile-state">{{ tile.state }}</span>
                <span class="tile-value">{{ tile.value }}</span>
              </div>
            </li>
            <li data-testid="tile-coverage" class="tile" :class="`tile--${coverageTile.tone}`">
              <div class="tile-body">
                <span class="tile-name">{{ coverageTile.name }}</span>
                <span class="tile-state">{{ coverageTile.state }}</span>
                <span class="tile-value">{{ coverageTile.value }}</span>
              </div>
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

      <section
        v-show="sectionVisible('workflows')"
        data-testid="section-workflows"
        class="card"
        aria-labelledby="wf-title"
      >
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

      <section
        v-show="sectionVisible('runs')"
        data-testid="section-runs"
        class="card"
        aria-labelledby="runs-title"
      >
        <h2 id="runs-title">Runs de cette sandbox</h2>
        <RunList :key="runsKey" :sandbox="sandboxName" @select="openRun" />
      </section>
    </div>
  </div>

  <nav
    v-if="stacked"
    class="tabbar"
    role="tablist"
    aria-label="Sections de la sandbox"
  >
    <button
      v-for="tab in tabs"
      :key="tab.id"
      type="button"
      role="tab"
      class="tab"
      :class="{ 'tab--active': activeTab === tab.id }"
      :aria-selected="activeTab === tab.id"
      :data-testid="`tab-${tab.id}`"
      @click="activeTab = tab.id"
    >
      <svg v-if="tab.id === 'health'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
        <path d="M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z" /><path d="M9 12l2 2 4-4" />
      </svg>
      <svg v-else-if="tab.id === 'workflows'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
        <rect x="4" y="4" width="6" height="6" rx="1" /><rect x="14" y="14" width="6" height="6" rx="1" /><path d="M10 7h2a2 2 0 0 1 2 2v5" />
      </svg>
      <svg v-else-if="tab.id === 'runs'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
        <path d="M8 5v14l11-7z" />
      </svg>
      <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
        <path d="M4 5h16v11H9l-5 4z" />
      </svg>
      <span>{{ tab.label }}</span>
    </button>
  </nav>

  <AssistantPanel
    :open="panelOpen"
    title="Assistant de la sandbox"
    @close="closeAssistant"
  >
    <ContextChat kind="sandbox" :data="{ sandbox_name: sandboxName }" @turn-finished="onTurnFinished" />
  </AssistantPanel>
  </div>
</template>

<style scoped>
/* Écran : une ligne (contenu + panneau d'assistant à partir de md). La page ne défile jamais. */
.screen {
  box-sizing: border-box;
  display: flex;
  height: 100%;
  min-width: 0;
}

.overview {
  box-sizing: border-box;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 16px;
  min-width: 0;
  height: 100%;
  padding: 16px;
  max-width: 100%;
  overflow-x: hidden;
  overflow-y: auto;
}

/* Sous md : la barre d'onglets de l'écran recouvre la barre du shell ; le contenu passe dessous. */
.overview--stacked {
  padding-bottom: calc(var(--nav-h) + 16px);
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
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
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

.header-row {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.header-main {
  flex: 1;
  min-width: 0;
}

.back {
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--ctl-h);
  height: var(--ctl-h);
  border-radius: 6px;
  color: var(--muted);
  text-decoration: none;
}

.back:hover {
  color: var(--text);
  background: var(--surface-2);
}

.back:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.back svg {
  width: 20px;
  height: 20px;
}

.phase-pill {
  flex: none;
  font-size: 0.8rem;
  padding: 2px 10px;
  border-radius: 999px;
  border: 1px solid currentColor;
}

.phase--ok {
  color: var(--ok);
}

.phase--muted {
  color: var(--muted);
}

.header-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 12px;
}

.meta {
  font-size: 0.9rem;
  color: var(--muted);
  margin: 2px 0;
}

.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: var(--ctl-h);
  min-width: var(--ctl-h);
  padding: 0 14px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-2);
  color: inherit;
  font: inherit;
  cursor: pointer;
  text-decoration: none;
}

.btn:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
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

.badge--ok { color: var(--ok); }
.badge--warn { color: var(--warn); }
.badge--busy { color: var(--accent); }

.synthesis {
  margin: 0 0 8px;
  font-size: 1.05rem;
  font-weight: 600;
}

.synthesis--ok { color: var(--ok); }
.synthesis--warn { color: var(--warn); }

.verdict--ok { color: var(--ok); }
.verdict--warn { color: var(--warn); }
.verdict--fail { color: var(--err); }

.warning {
  margin: 8px 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--warn) 15%, transparent);
  color: var(--warn);
}

/* Vignettes de vérification : deux colonnes, état en texte + point de couleur. */
.tiles {
  list-style: none;
  padding: 0;
  margin: 8px 0;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.tile {
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-2);
}

.tile-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-height: var(--ctl-h);
  padding: 8px 10px;
}

.tile-name {
  font-size: 0.8rem;
  color: var(--muted);
  overflow-wrap: anywhere;
}

.tile-state {
  font-size: 0.8rem;
  font-weight: 600;
}

.tile-state::before {
  content: '';
  display: inline-block;
  width: 8px;
  height: 8px;
  margin-right: 6px;
  border-radius: 50%;
  background: currentColor;
}

.tile-value {
  font-size: 0.95rem;
  font-weight: 600;
}

.tile--ok .tile-state { color: var(--ok); }
.tile--warn .tile-state { color: var(--warn); }
.tile--fail .tile-state { color: var(--err); }
.tile--muted .tile-state { color: var(--muted); }

.workflows,
.files,
.issues ul {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}

.workflow {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  padding: 6px 0;
  border-bottom: 1px solid var(--border);
}

.file-path,
.issue-path {
  display: inline-block;
  max-width: 100%;
  font-family: var(--font-mono);
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
  color: var(--muted);
}

.issues summary {
  min-height: var(--ctl-h);
  display: flex;
  align-items: center;
  cursor: pointer;
}

.issue {
  padding: 6px 0;
  border-bottom: 1px solid var(--border);
}

.actions {
  margin-top: 12px;
}

.ide-unavailable p {
  font-size: 0.85rem;
  color: var(--muted);
}

/* Barre d'onglets de l'écran (sous md). Position fixe au-dessus de la barre du shell, qu'elle recouvre. */
.tabbar {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 30;
  height: var(--nav-h);
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  background: var(--surface);
  border-top: 1px solid var(--border);
}

.tab {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  min-height: var(--ctl-h);
  padding: 4px 2px;
  border: none;
  background: transparent;
  color: var(--muted);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}

.tab svg {
  width: 20px;
  height: 20px;
}

.tab--active {
  color: var(--accent);
}

.tab:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}

@media (min-width: 768px) {
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
    align-items: start;
  }
}
</style>
