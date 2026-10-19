import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { RunDetail, RunState } from '@vanyline/protocol';
import type { WorkflowsClient } from '../../api/workflowsClient';
import RunDetailView from './RunDetail.vue';

function fakeClient(overrides: Partial<WorkflowsClient> = {}): WorkflowsClient {
  return {
    listWorkflows: vi.fn(),
    getWorkflow: vi.fn(),
    createWorkflow: vi.fn(),
    updateWorkflow: vi.fn(),
    deleteWorkflow: vi.fn(),
    launchWorkflow: vi.fn(),
    listProjectRuns: vi.fn(),
    listSandboxRuns: vi.fn(),
    getRun: vi.fn(),
    cancelRun: vi.fn(async () => undefined),
    getCodeHealth: vi.fn(),
    refreshCodeHealth: vi.fn(),
    ...overrides,
  } as WorkflowsClient;
}

function detail(state: RunState, steps: RunDetail['status']['steps'] = []): RunDetail {
  return {
    run_id: 'r1',
    workflow: 'owner/demo/build',
    scope: 'project',
    triggered_by: 'ui',
    started_at: '2026-10-17T10:00:00Z',
    status: {
      invocation_id: 'inv-1',
      state,
      started_at: '2026-10-17T10:00:00Z',
      ended_at: state === 'Running' ? null : '2026-10-17T10:05:00Z',
      error: null,
      steps,
    },
  };
}

afterEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function buttonByText(root: ParentNode, text: string): HTMLButtonElement | undefined {
  return Array.from(root.querySelectorAll('button')).find(
    (b) => b.textContent?.trim() === text,
  ) as HTMLButtonElement | undefined;
}

describe('RunDetail', () => {
  it('detail_d_un_run', async () => {
    const client = fakeClient({
      getRun: vi.fn(async () =>
        detail('Failed', [
          { step_id: 'a', state: 'Succeeded', started_at: null, ended_at: null, error: null },
          { step_id: 'b', state: 'Failed', started_at: null, ended_at: null, error: 'boom: exit 2' },
        ]),
      ),
    });
    const wrapper = mount(RunDetailView, { props: { runId: 'r1', client } });
    await flushPromises();

    expect(client.getRun).toHaveBeenCalledWith('r1');
    expect(wrapper.text()).toContain('Failed');
    const rowB = wrapper.findAll('tr').find((tr) => tr.text().includes('b'));
    expect(rowB!.text()).toContain('boom: exit 2');
  });

  it('run_purge', async () => {
    const client = fakeClient({ getRun: vi.fn(async () => detail('Unknown')) });
    const wrapper = mount(RunDetailView, { props: { runId: 'r1', client } });
    await flushPromises();

    expect(wrapper.text()).toContain('purgé du moteur');
    expect(buttonByText(wrapper.element, 'Annuler le run')).toBeUndefined();
  });

  it('annulation_confirmee', async () => {
    const client = fakeClient({ getRun: vi.fn(async () => detail('Running')) });
    const wrapper = mount(RunDetailView, { props: { runId: 'r1', client } });
    await flushPromises();

    buttonByText(wrapper.element, 'Annuler le run')!.click();
    await flushPromises();

    // Confirmation d'abord : aucun appel tant qu'elle n'est pas donnée.
    expect(client.cancelRun).not.toHaveBeenCalled();
    const dialog = document.body.querySelector('[role="dialog"]') as HTMLElement;
    expect(dialog).not.toBeNull();

    buttonByText(dialog, "Confirmer l'annulation")!.click();
    await flushPromises();

    expect(client.cancelRun).toHaveBeenCalledWith('r1');
  });

  it('env_masque', async () => {
    // Les steps de run n'exposent pas leur config ; on simule une fuite éventuelle.
    const leaked = {
      step_id: 'build',
      state: 'Succeeded',
      started_at: null,
      ended_at: null,
      error: null,
      env: { TOKEN: 'sekret-42' },
    } as unknown as RunDetail['status']['steps'][number];
    const client = fakeClient({ getRun: vi.fn(async () => detail('Succeeded', [leaked])) });
    const wrapper = mount(RunDetailView, { props: { runId: 'r1', client } });
    await flushPromises();

    expect(wrapper.text()).toContain('build');
    expect(wrapper.html()).not.toContain('sekret-42');
    expect(wrapper.html()).not.toContain('TOKEN');
  });
});
