import type {
  HealthState,
  RunDetail,
  RunLaunch,
  RunSummary,
  Workflow,
  WorkflowCreate,
  WorkflowSummary,
  WorkflowUpdate,
} from '@vanyline/protocol';
import { ApiError } from './client';
import type { ApiClient } from './client';

/** Moteur de workflow indisponible (`503` : non configuré ou Restate injoignable). */
export class EngineUnavailable extends ApiError {
  constructor(message: string, code?: string) {
    super(503, code, message);
    this.name = 'EngineUnavailable';
  }
}

/** Fenêtre de listes de runs. Bornes appliquées côté API (limit ≤ 50, since en jours ≤ 14). */
export interface RunsWindow {
  limit?: number;
  /** Entier de jours (défaut API : 14). */
  since?: number;
}

export interface WorkflowsClient {
  listWorkflows(project: string): Promise<WorkflowSummary[]>;
  getWorkflow(project: string, id: number): Promise<Workflow>;
  createWorkflow(project: string, body: WorkflowCreate): Promise<void>;
  updateWorkflow(project: string, id: number, body: WorkflowUpdate): Promise<void>;
  deleteWorkflow(project: string, id: number): Promise<void>;
  launchWorkflow(project: string, id: number, opts?: { sandbox?: string }): Promise<RunLaunch>;
  listProjectRuns(project: string, opts?: RunsWindow): Promise<RunSummary[]>;
  listSandboxRuns(sandbox: string, opts?: RunsWindow): Promise<RunSummary[]>;
  getRun(runId: string): Promise<RunDetail>;
  cancelRun(runId: string): Promise<void>;
  getCodeHealth(sandbox: string): Promise<HealthState>;
  /** `base` absente = base sticky inchangée ; `null` = efface la base. */
  refreshCodeHealth(sandbox: string, opts?: { base?: string | null }): Promise<void>;
}

const projectBase = (project: string): string =>
  `/api/projects/${encodeURIComponent(project)}`;

const sandboxBase = (sandbox: string): string =>
  `/api/sandboxes/${encodeURIComponent(sandbox)}`;

function windowQuery(opts?: RunsWindow): string {
  const params = new URLSearchParams();
  if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
  if (opts?.since !== undefined) params.set('since', String(opts.since));
  const qs = params.toString();
  return qs ? `?${qs}` : '';
}

/** Traduit le `503` en `EngineUnavailable` ; les autres erreurs gardent leur `ApiError`. */
async function guard<T>(pending: Promise<T>): Promise<T> {
  try {
    return await pending;
  } catch (err) {
    if (err instanceof ApiError && err.status === 503) {
      throw new EngineUnavailable(err.message, err.code);
    }
    throw err;
  }
}

/** Client des workflows, des runs et de la santé du code (relayée par l'app). */
export function createWorkflowsClient(client: ApiClient): WorkflowsClient {
  return {
    listWorkflows(project) {
      return guard(client.get<WorkflowSummary[]>(`${projectBase(project)}/workflows`));
    },

    getWorkflow(project, id) {
      return guard(client.get<Workflow>(`${projectBase(project)}/workflows/${id}`));
    },

    createWorkflow(project, body) {
      return guard(client.post<void>(`${projectBase(project)}/workflows`, body));
    },

    updateWorkflow(project, id, body) {
      return guard(client.put<void>(`${projectBase(project)}/workflows/${id}`, body));
    },

    deleteWorkflow(project, id) {
      return guard(client.delete(`${projectBase(project)}/workflows/${id}`));
    },

    launchWorkflow(project, id, opts = {}) {
      return guard(
        client.post<RunLaunch>(`${projectBase(project)}/workflows/${id}/runs`, {
          sandbox: opts.sandbox,
        }),
      );
    },

    listProjectRuns(project, opts) {
      return guard(
        client.get<RunSummary[]>(`${projectBase(project)}/runs${windowQuery(opts)}`),
      );
    },

    listSandboxRuns(sandbox, opts) {
      return guard(
        client.get<RunSummary[]>(`${sandboxBase(sandbox)}/runs${windowQuery(opts)}`),
      );
    },

    getRun(runId) {
      return guard(client.get<RunDetail>(`/api/runs/${encodeURIComponent(runId)}`));
    },

    cancelRun(runId) {
      return guard(client.post<void>(`/api/runs/${encodeURIComponent(runId)}/cancel`));
    },

    getCodeHealth(sandbox) {
      return guard(client.get<HealthState>(`${sandboxBase(sandbox)}/code-health`));
    },

    refreshCodeHealth(sandbox, opts = {}) {
      return guard(
        client.post<void>(`${sandboxBase(sandbox)}/code-health/refresh`, opts),
      );
    },
  };
}
