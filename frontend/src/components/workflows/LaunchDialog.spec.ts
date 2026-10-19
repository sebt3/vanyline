import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { ApiError } from '../../api/client';
import type { WorkflowsClient } from '../../api/workflowsClient';
import LaunchDialog from './LaunchDialog.vue';

function fakeClient(overrides: Partial<WorkflowsClient> = {}): WorkflowsClient {
  return {
    listWorkflows: vi.fn(),
    getWorkflow: vi.fn(),
    createWorkflow: vi.fn(),
    updateWorkflow: vi.fn(),
    deleteWorkflow: vi.fn(),
    launchWorkflow: vi.fn(async () => ({ run_id: 'r1', invocation_id: 'inv-1' })),
    listProjectRuns: vi.fn(),
    listSandboxRuns: vi.fn(),
    getRun: vi.fn(),
    cancelRun: vi.fn(),
    getCodeHealth: vi.fn(),
    refreshCodeHealth: vi.fn(),
    ...overrides,
  } as WorkflowsClient;
}

afterEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function dialogEl(): HTMLElement {
  const el = document.body.querySelector<HTMLElement>('[role="dialog"]');
  if (!el) throw new Error('dialogue absent');
  return el;
}

describe('LaunchDialog', () => {
  it('lancement_portee_sandbox', async () => {
    const client = fakeClient();
    mount(LaunchDialog, {
      props: {
        open: true,
        project: 'demo',
        workflow: { id: 3, name: 'build' },
        scope: 'sandbox',
        sandboxes: [
          { name: 's-running', phase: 'Running' },
          { name: 's-stopped', phase: 'Suspended' },
        ],
        client,
      },
    });
    await flushPromises();

    const options = Array.from(dialogEl().querySelectorAll('option')).map((o) => o.value);
    expect(options).toEqual(['s-running']);
  });

  it('lance_sur_la_sandbox_choisie', async () => {
    const client = fakeClient();
    const wrapper = mount(LaunchDialog, {
      props: {
        open: true,
        project: 'demo',
        workflow: { id: 3, name: 'build' },
        scope: 'sandbox',
        sandboxes: [{ name: 's-running', phase: 'Running' }],
        client,
      },
    });
    await flushPromises();

    const launch = Array.from(dialogEl().querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Lancer',
    );
    launch!.click();
    await flushPromises();

    expect(client.launchWorkflow).toHaveBeenCalledWith('demo', 3, { sandbox: 's-running' });
    expect(wrapper.emitted('launched')?.[0]).toEqual([
      { run_id: 'r1', invocation_id: 'inv-1' },
    ]);
  });

  it('sans sandbox Running, le lancement est désactivé', async () => {
    mount(LaunchDialog, {
      props: {
        open: true,
        project: 'demo',
        workflow: { id: 3, name: 'build' },
        scope: 'sandbox',
        sandboxes: [{ name: 's-stopped', phase: 'Suspended' }],
        client: fakeClient(),
      },
    });
    await flushPromises();

    const launch = Array.from(dialogEl().querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Lancer',
    );
    expect((launch as HTMLButtonElement).disabled).toBe(true);
  });

  it('portee_projet_confirmation_seule', async () => {
    const client = fakeClient();
    mount(LaunchDialog, {
      props: {
        open: true,
        project: 'demo',
        workflow: { id: 7, name: 'lint' },
        scope: 'project',
        sandboxes: [],
        client,
      },
    });
    await flushPromises();

    expect(dialogEl().querySelector('select')).toBeNull();
    const launch = Array.from(dialogEl().querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Lancer',
    );
    launch!.click();
    await flushPromises();

    expect(client.launchWorkflow).toHaveBeenCalledWith('demo', 7, { sandbox: undefined });
  });

  it('affiche_l_erreur_de_l_api', async () => {
    const client = fakeClient({
      launchWorkflow: vi.fn(async () => {
        throw new ApiError(422, 'VNL-WFA-422', 'VNL-WFA-422: kind interdit');
      }),
    });
    mount(LaunchDialog, {
      props: {
        open: true,
        project: 'demo',
        workflow: { id: 7, name: 'lint' },
        scope: 'project',
        sandboxes: [],
        client,
      },
    });
    await flushPromises();

    const launch = Array.from(dialogEl().querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Lancer',
    );
    launch!.click();
    await flushPromises();

    expect(dialogEl().textContent).toContain('VNL-WFA-422: kind interdit');
  });
});
