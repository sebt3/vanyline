import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { RunSummary, RunState } from '@vanyline/protocol';
import type { WorkflowsClient } from '../../api/workflowsClient';
import RunList from './RunList.vue';

function fakeClient(overrides: Partial<WorkflowsClient> = {}): WorkflowsClient {
  return {
    listWorkflows: vi.fn(),
    getWorkflow: vi.fn(),
    createWorkflow: vi.fn(),
    updateWorkflow: vi.fn(),
    deleteWorkflow: vi.fn(),
    launchWorkflow: vi.fn(),
    listProjectRuns: vi.fn(async () => []),
    listSandboxRuns: vi.fn(async () => []),
    getRun: vi.fn(),
    cancelRun: vi.fn(),
    getCodeHealth: vi.fn(),
    refreshCodeHealth: vi.fn(),
    ...overrides,
  } as WorkflowsClient;
}

function run(id: string, state: RunState): RunSummary {
  return {
    run_id: id,
    workflow: `owner/demo/build-${id}`,
    scope: 'project',
    triggered_by: 'ui',
    started_at: '2026-10-17T10:00:00Z',
    status: {
      invocation_id: `inv-${id}`,
      state,
      started_at: '2026-10-17T10:00:00Z',
      ended_at: null,
      error: null,
    },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

describe('RunList', () => {
  it('run_en_cours', async () => {
    const client = fakeClient({
      listProjectRuns: vi
        .fn()
        .mockResolvedValueOnce([run('r1', 'Running')])
        .mockResolvedValueOnce([run('r1', 'Succeeded')])
        .mockResolvedValue([run('r1', 'Succeeded')]),
    });
    mount(RunList, { props: { project: 'demo', client } });
    await vi.advanceTimersByTimeAsync(0);
    expect(client.listProjectRuns).toHaveBeenCalledTimes(1);
    expect(client.listProjectRuns).toHaveBeenCalledWith('demo', { limit: 50, since: 14 });

    // Un run est actif : interrogation toutes les 10 s.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(client.listProjectRuns).toHaveBeenCalledTimes(2);

    // Le run passe en `Succeeded` à la 2e réponse : plus aucun run actif, l'interrogation s'arrête.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(client.listProjectRuns).toHaveBeenCalledTimes(2);
  });

  it('run_en_cours — aucun run actif : pas d interrogation', async () => {
    const client = fakeClient({
      listSandboxRuns: vi.fn(async () => [run('r9', 'Failed')]),
    });
    mount(RunList, { props: { sandbox: 's1', client } });
    await vi.advanceTimersByTimeAsync(30_000);

    expect(client.listSandboxRuns).toHaveBeenCalledTimes(1);
    expect(client.listSandboxRuns).toHaveBeenCalledWith('s1', { limit: 50, since: 14 });
  });

  it('cartes_sous_md', async () => {
    const client = fakeClient({
      listProjectRuns: vi.fn(async () => [run('r1', 'Succeeded'), run('r2', 'Failed')]),
    });
    const wrapper = mount(RunList, { props: { project: 'demo', client } });
    await flushPromises();

    // Tableau et cartes coexistent ; la bascule se fait en CSS à partir de md.
    expect(wrapper.find('.run-table').exists()).toBe(true);
    expect(wrapper.findAll('.run-card')).toHaveLength(2);
  });

  it('env_masque', async () => {
    // Un run ne porte pas de config de step ; on simule une fuite éventuelle.
    const leaked = { ...run('r1', 'Running'), env: { TOKEN: 'sekret-42' } } as RunSummary;
    const client = fakeClient({ listProjectRuns: vi.fn(async () => [leaked]) });
    const wrapper = mount(RunList, { props: { project: 'demo', client } });
    await flushPromises();

    expect(wrapper.html()).not.toContain('sekret-42');
    expect(wrapper.html()).not.toContain('TOKEN');
  });

  it('emet select au clic sur un run', async () => {
    const client = fakeClient({
      listProjectRuns: vi.fn(async () => [run('r1', 'Succeeded')]),
    });
    const wrapper = mount(RunList, { props: { project: 'demo', client } });
    await flushPromises();

    const btn = wrapper.findAll('button').find((b) => b.text() === 'Détail');
    await btn!.trigger('click');

    expect(wrapper.emitted('select')?.[0]).toEqual(['r1']);
  });
});
