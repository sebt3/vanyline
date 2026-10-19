import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { WorkflowSummary } from '@vanyline/protocol';
import type { WorkflowsClient } from '../../api/workflowsClient';
import WorkflowList from './WorkflowList.vue';

function fakeClient(overrides: Partial<WorkflowsClient> = {}): WorkflowsClient {
  return {
    listWorkflows: vi.fn(async () => []),
    getWorkflow: vi.fn(),
    createWorkflow: vi.fn(),
    updateWorkflow: vi.fn(),
    deleteWorkflow: vi.fn(async () => undefined),
    launchWorkflow: vi.fn(),
    listProjectRuns: vi.fn(),
    listSandboxRuns: vi.fn(),
    getRun: vi.fn(),
    cancelRun: vi.fn(),
    getCodeHealth: vi.fn(),
    refreshCodeHealth: vi.fn(),
    ...overrides,
  } as WorkflowsClient;
}

const projectWf: WorkflowSummary = {
  id: 1,
  name: 'lint-projet',
  scope: 'project',
  kinds: ['agent', 'rhai'],
  steps_count: 2,
};

const sandboxWf: WorkflowSummary = {
  id: 2,
  name: 'build-sandbox',
  scope: 'sandbox',
  kinds: ['shell', 'gate'],
  steps_count: 3,
};

afterEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

describe('WorkflowList', () => {
  it('env_masque', async () => {
    // Un step shell porte un `env` secret côté serveur ; la liste ne doit
    // jamais l'afficher. Le résumé ne le transporte pas, mais on simule une
    // fuite éventuelle en ajoutant le champ à la réponse.
    const leaked = { ...sandboxWf, env: { TOKEN: 'sekret-42' } } as WorkflowSummary;
    const client = fakeClient({ listWorkflows: vi.fn(async () => [leaked]) });

    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    expect(wrapper.text()).toContain('build-sandbox');
    expect(wrapper.html()).not.toContain('sekret-42');
    expect(wrapper.html()).not.toContain('TOKEN');
  });

  it('lancement_emet_le_workflow', async () => {
    const client = fakeClient({ listWorkflows: vi.fn(async () => [projectWf]) });
    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    const launch = wrapper.findAll('button').find((b) => b.text() === 'Lancer');
    await launch!.trigger('click');

    expect(wrapper.emitted('launch')?.[0]).toEqual([projectWf]);
  });

  it('filtre_par_portee', async () => {
    const client = fakeClient({
      listWorkflows: vi.fn(async () => [projectWf, sandboxWf]),
    });
    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    const sandboxFilter = wrapper.findAll('button').find((b) => b.text() === 'Sandbox');
    await sandboxFilter!.trigger('click');

    expect(wrapper.text()).toContain('build-sandbox');
    expect(wrapper.text()).not.toContain('lint-projet');
  });

  it('suppression_confirmee', async () => {
    const client = fakeClient({ listWorkflows: vi.fn(async () => [projectWf]) });
    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    const del = wrapper.findAll('button').find((b) => b.text() === 'Supprimer');
    await del!.trigger('click');
    await flushPromises();

    // Pas d'appel tant que la confirmation n'est pas donnée.
    expect(client.deleteWorkflow).not.toHaveBeenCalled();
    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();

    const confirm = Array.from(dialog!.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Supprimer',
    );
    confirm!.click();
    await flushPromises();

    expect(client.deleteWorkflow).toHaveBeenCalledWith('demo', 1);
    expect(wrapper.emitted('delete')?.[0]).toEqual([1]);
  });

  it('cartes_sous_md', async () => {
    const client = fakeClient({
      listWorkflows: vi.fn(async () => [projectWf, sandboxWf]),
    });
    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    // Les deux rendus coexistent ; la bascule tableau/cartes se fait en CSS (md).
    expect(wrapper.find('.wf-table').exists()).toBe(true);
    expect(wrapper.findAll('.wf-card')).toHaveLength(2);
  });

  it('erreur_moteur_indisponible', async () => {
    const { EngineUnavailable } = await import('../../api/workflowsClient');
    const client = fakeClient({
      listWorkflows: vi.fn(async () => {
        throw new EngineUnavailable('Restate injoignable');
      }),
    });
    const wrapper = mount(WorkflowList, { props: { project: 'demo', client } });
    await flushPromises();

    expect(wrapper.text()).toContain('Moteur de workflow indisponible.');
  });
});

describe('WorkflowList (lecture seule, portée figée)', () => {
  it('readonly_sans_actions_de_mutation : pas de création, édition ni suppression, le lancement reste', async () => {
    const wrapper = mount(WorkflowList, {
      props: { project: 'demo', readonly: true, client: fakeClient({ listWorkflows: vi.fn(async () => [sandboxWf]) }) },
    });
    await flushPromises();

    const labels = wrapper.findAll('button').map((b) => b.text());
    expect(labels).not.toContain('Nouveau workflow');
    expect(labels).not.toContain('Éditer');
    expect(labels).not.toContain('Supprimer');
    expect(labels).toContain('Lancer');
  });

  it('readonly_faux_par_defaut : les actions de mutation sont présentes sans la prop', async () => {
    const wrapper = mount(WorkflowList, {
      props: { project: 'demo', client: fakeClient({ listWorkflows: vi.fn(async () => [sandboxWf]) }) },
    });
    await flushPromises();

    const labels = wrapper.findAll('button').map((b) => b.text());
    expect(labels).toContain('Nouveau workflow');
    expect(labels).toContain('Éditer');
    expect(labels).toContain('Supprimer');
  });

  it('portee_figee_sans_filtre : fixedScope liste seulement cette portée et masque le filtre', async () => {
    const wrapper = mount(WorkflowList, {
      props: {
        project: 'demo',
        fixedScope: 'sandbox',
        client: fakeClient({ listWorkflows: vi.fn(async () => [projectWf, sandboxWf]) }),
      },
    });
    await flushPromises();

    expect(wrapper.text()).toContain('build-sandbox');
    expect(wrapper.text()).not.toContain('lint-projet');
    expect(wrapper.find('[role="group"][aria-label="Filtrer par portée"]').exists()).toBe(false);
  });
});
