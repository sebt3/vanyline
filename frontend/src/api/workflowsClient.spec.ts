import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { ApiError, createApiClient } from './client';
import { EngineUnavailable, createWorkflowsClient } from './workflowsClient';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('workflowsClient', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;
  const seenUrls: string[] = [];

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockReset();
    seenUrls.length = 0;
  });

  afterEach(() => {
    fetchSpy.mockRestore();
  });

  /** Faux serveur : répond `body`/`status` et enregistre chaque URL vue. */
  function serve(body: unknown, status = 200): void {
    fetchSpy.mockImplementation(async (url: string) => {
      seenUrls.push(url);
      return jsonResponse(body, status);
    });
  }

  function lastInit(): RequestInit {
    return fetchSpy.mock.calls[fetchSpy.mock.calls.length - 1][1] as RequestInit;
  }

  it('lancer_un_workflow', async () => {
    serve({ run_id: 'r1', invocation_id: 'inv-1' }, 202);
    const wf = createWorkflowsClient(createApiClient());

    const launched = await wf.launchWorkflow('demo', 3, { sandbox: 's1' });

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/projects/demo/workflows/3/runs',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sandbox: 's1' }),
      }),
    );
    expect(launched).toEqual({ run_id: 'r1', invocation_id: 'inv-1' });
  });

  it('sante_relayee', async () => {
    const health = { phase: 'fresh', report: null, computed_at: 1760000000000 };
    serve(health);
    const wf = createWorkflowsClient(createApiClient());

    const result = await wf.getCodeHealth('s1');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s1/code-health',
      expect.objectContaining({ method: 'GET', credentials: 'include' }),
    );
    expect(result).toEqual(health);
  });

  it('moteur_indisponible', async () => {
    serve({ error: 'VNL-WFA-503: moteur non configuré' }, 503);
    const wf = createWorkflowsClient(createApiClient());

    const err = await wf.listWorkflows('demo').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(EngineUnavailable);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(503);
  });

  it('moteur_indisponible — 503 sur un appel de run aussi', async () => {
    serve({ error: 'Restate injoignable' }, 503);
    const wf = createWorkflowsClient(createApiClient());

    await expect(wf.getRun('r1')).rejects.toBeInstanceOf(EngineUnavailable);
  });

  it('erreur non-503 garde son statut (ApiError, pas EngineUnavailable)', async () => {
    serve({ error: 'VNL-WFA-422: kind interdit' }, 422);
    const wf = createWorkflowsClient(createApiClient());

    const err = await wf.launchWorkflow('demo', 3).catch((e: unknown) => e);

    expect(err).toBeInstanceOf(ApiError);
    expect(err).not.toBeInstanceOf(EngineUnavailable);
    expect((err as ApiError).status).toBe(422);
    expect((err as ApiError).code).toBe('VNL-WFA-422');
  });

  it('fenetre_des_runs', async () => {
    serve([]);
    const wf = createWorkflowsClient(createApiClient());

    await wf.listProjectRuns('demo', { limit: 50, since: 7 });

    const url = fetchSpy.mock.calls[0][0] as string;
    expect(url.startsWith('/api/projects/demo/runs?')).toBe(true);
    const params = new URL(url, 'http://x').searchParams;
    expect(params.get('limit')).toBe('50');
    expect(params.get('since')).toBe('7');
  });

  it('fenetre_des_runs — sans options, aucune query ajoutée', async () => {
    serve([]);
    const wf = createWorkflowsClient(createApiClient());

    await wf.listSandboxRuns('s1');

    expect(fetchSpy.mock.calls[0][0]).toBe('/api/sandboxes/s1/runs');
  });

  it('aucun_appel_direct', async () => {
    serve({});
    const wf = createWorkflowsClient(createApiClient());

    await wf.listWorkflows('demo');
    await wf.getWorkflow('demo', 3);
    await wf.deleteWorkflow('demo', 3);
    await wf.launchWorkflow('demo', 3, { sandbox: 's1' });
    await wf.listProjectRuns('demo');
    await wf.listSandboxRuns('s1');
    await wf.getRun('r1');
    await wf.cancelRun('r1');
    await wf.getCodeHealth('s1');
    await wf.refreshCodeHealth('s1', { base: 'main' });

    expect(seenUrls.length).toBeGreaterThan(0);
    for (const url of seenUrls) {
      expect(url.startsWith('/api/'), url).toBe(true);
    }
  });

  it('refreshCodeHealth : base absente = corps sans base, null passe tel quel', async () => {
    serve({}, 202);
    const wf = createWorkflowsClient(createApiClient());

    await wf.refreshCodeHealth('s1');
    expect(lastInit().body).toBe('{}');

    await wf.refreshCodeHealth('s1', { base: null });
    expect(lastInit().body).toBe(JSON.stringify({ base: null }));
  });

  it('cancelRun émet POST /api/runs/{id}/cancel', async () => {
    serve({}, 202);
    const wf = createWorkflowsClient(createApiClient());

    await wf.cancelRun('run 1');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/runs/run%201/cancel',
      expect.objectContaining({ method: 'POST' }),
    );
  });

  it('reponse_sans_corps', async () => {
    fetchSpy.mockImplementation(async () => new Response(null, { status: 202, headers: { 'content-length': '0' } }));
    const wf = createWorkflowsClient(createApiClient());

    await expect(wf.cancelRun('r1')).resolves.toBeUndefined();

    fetchSpy.mockImplementation(async () => new Response(null, { status: 204 }));
    await expect(wf.deleteWorkflow('demo', 3)).resolves.toBeUndefined();
  });

  it('deleteWorkflow émet DELETE /api/projects/{p}/workflows/{id}', async () => {
    fetchSpy.mockImplementation(async () => new Response(null, { status: 204 }));
    const wf = createWorkflowsClient(createApiClient());

    await wf.deleteWorkflow('demo', 3);

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/projects/demo/workflows/3',
      expect.objectContaining({ method: 'DELETE', credentials: 'include' }),
    );
  });
});
