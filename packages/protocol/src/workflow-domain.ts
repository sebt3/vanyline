// Miroir TypeScript manuel de l'API des workflows (app/src/api/workflows.sdd) et des
// statuts de run (app/src/workflow/status.sdd). Noms snake_case = JSON de l'app.
// DO NOT drift — cf. workflow-domain.conformance.spec.ts.
// La santé du code est GÉNÉRÉE (ts-rs) : importée, jamais redéfinie ici.

import type { CheckStatus, HealthReport } from './generated/health-report';

export type { CheckStatus, HealthReport } from './generated/health-report';

/** Portée d'un workflow : `project` (sans sandbox) ou `sandbox` (lancé sur une sandbox du projet). */
export type WorkflowScope = 'project' | 'sandbox';

/** Kinds de step, fermés. Miroir de `./workflow/steps/mod.sdd`. */
export const STEP_KINDS = ['agent', 'rhai', 'shell', 'gate'] as const;

/** Configuration du kind `agent` (./workflow/steps/agent.sdd). `_run` est injecté côté serveur. */
export interface AgentStepConfig {
  agent: string;
  prompt: string;
  timeout_secs?: number;
  json?: boolean;
  max_steps?: number;
}

/** Configuration du kind `rhai` (./workflow/steps/rhai.sdd). */
export interface RhaiStepConfig {
  script: string;
}

/** Configuration du kind `shell` (./workflow/steps/shell.sdd). Réservé à la portée sandbox. */
export interface ShellStepConfig {
  script: string;
  cwd?: string;
  env?: Record<string, string>;
  timeout_secs?: number;
  allow_failure?: boolean;
}

/** Configuration du kind `gate` (./workflow/steps/gate.sdd). `null` désactive un seuil. */
export interface GateStepConfig {
  min_coverage?: number | null;
  min_new_code_coverage?: number | null;
  base?: string;
  max_errors?: number | null;
  ignore_checks?: string[];
  fail_on_unchecked?: boolean;
  enforce?: boolean;
  wait_timeout_secs?: number;
}

interface StepDefinitionBase {
  id: string;
  depends_on: string[];
}

/** Step d'un DAG, discriminé par `kind`. */
export type StepDefinition =
  | (StepDefinitionBase & { kind: 'agent'; config: AgentStepConfig })
  | (StepDefinitionBase & { kind: 'rhai'; config: RhaiStepConfig })
  | (StepDefinitionBase & { kind: 'shell'; config: ShellStepConfig })
  | (StepDefinitionBase & { kind: 'gate'; config: GateStepConfig });

/** Élément de `GET /projects/{project}/workflows`. */
export interface WorkflowSummary {
  id: number;
  name: string;
  scope: WorkflowScope;
  description?: string;
  kinds: string[];
  steps_count: number;
}

/** `GET /projects/{project}/workflows/{id}` : résumé + steps. */
export interface Workflow extends WorkflowSummary {
  steps: StepDefinition[];
}

/** Corps de `POST /projects/{project}/workflows`. */
export interface WorkflowCreate {
  name: string;
  scope: WorkflowScope;
  description?: string;
  steps: StepDefinition[];
}

/** Corps de `PUT /projects/{project}/workflows/{id}` (la portée n'est pas modifiable). */
export interface WorkflowUpdate {
  description?: string;
  steps?: StepDefinition[];
}

/** État d'un run tel que lu dans Restate. `Unknown` = invocation purgée. */
export type RunState =
  | 'Pending'
  | 'Running'
  | 'Suspended'
  | 'BackingOff'
  | 'Succeeded'
  | 'Failed'
  | 'Cancelled'
  | 'Unknown';

/** Statut d'un step (journal Restate). */
export interface StepStatus {
  step_id: string;
  state: string;
  started_at: string | null;
  ended_at: string | null;
  error: string | null;
}

/** Statut sans le détail des steps (forme des listes de runs). */
export interface RunStatusHeader {
  invocation_id: string;
  state: RunState;
  started_at: string | null;
  ended_at: string | null;
  error: string | null;
}

/** Statut complet d'un run (`GET /runs/{run_id}`). */
export interface RunStatus extends RunStatusHeader {
  steps: StepStatus[];
}

/** Élément des listes de runs (`GET /projects/{project}/runs`, `GET /sandboxes/{sandbox}/runs`). */
export interface RunSummary {
  run_id: string;
  /** Nom complet stocké au lancement (`{owner}/{project}/{name}`). */
  workflow: string;
  scope: WorkflowScope;
  sandbox?: string;
  triggered_by: string;
  started_at: string;
  status: RunStatusHeader;
}

/** `GET /runs/{run_id}` : résumé + statut avec steps. */
export interface RunDetail extends RunSummary {
  status: RunStatus;
}

/** Réponse 202 de `POST /projects/{project}/workflows/{id}/runs`. */
export interface RunLaunch {
  run_id: string;
  invocation_id: string;
}

/** Phase de la santé du code, relayée telle quelle depuis la sandbox. */
export type HealthPhase = 'computing' | 'fresh' | 'stale';

/** Couverture d'un fichier dans le résumé ; `percent` trié croissant (le moins couvert d'abord). */
export interface HealthSummaryFile {
  path: string;
  percent: number;
}

/** Résumé dérivé par la sandbox (`@HealthSummary`, sandbox/src/health.sdd) : jamais recalculé côté
 *  consommateur. `coverage_percent` `null` = pas de mesure (≠ 0 %). */
export interface HealthSummary {
  status: CheckStatus;
  coverage_percent: number | null;
  files: HealthSummaryFile[];
}

/** `GET /sandboxes/{name}/code-health`. Horodatages : millisecondes unix (u64 côté sandbox).
 *  `summary` est `null` exactement quand `report` est `null`. */
export interface HealthState {
  phase: HealthPhase;
  report: HealthReport | null;
  summary: HealthSummary | null;
  computed_at?: number | null;
  stale_since?: number | null;
  base?: string | null;
}
