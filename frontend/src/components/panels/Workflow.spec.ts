import { flushPromises, mount } from '@vue/test-utils';
import { ref } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunSummary, WorkflowSummary } from '@vanyline/protocol';
import Workflow from './Workflow.vue';
import WorkflowList from '../workflows/WorkflowList.vue';

// Phases de sandbox (WebSocket d'états) : un Map fixé par test, avant le montage.
const { sandboxPhases } = vi.hoisted(() => ({
  sandboxPhases: { value: new Map<string, string>() },
}));

vi.mock('../../composables/useSandboxState', () => ({
  useSandboxState: () => ({ phases: sandboxPhases, connected: { value: true } }),
}));

interface Call {
  method: string;
  url: string;
  body: unknown;
}

type Reply = { status?: number; body?: unknown };
type Handler = (method: string, url: string, body: unknown) => Reply | undefined;

let calls: Call[] = [];

function mockApi(handler: Handler) {
  calls = [];
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? 'GET').toUpperCase();
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    calls.push({ method, url, body });
    const reply = handler(method, url, body) ?? { status: 404, body: { error: 'non mocké' } };
    const status = reply.status ?? 200;
    return {
      ok: status < 400,
      status,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve(reply.body),
    } as unknown as Response;
  });
}

const WORKFLOWS: WorkflowSummary[] = [
  { id: 1, name: 'lint-sandbox', scope: 'sandbox', kinds: ['shell'], steps_count: 1 },
  { id: 2, name: 'tests-sandbox', scope: 'sandbox', kinds: ['gate'], steps_count: 2 },
  { id: 3, name: 'publier-projet', scope: 'project', kinds: ['agent'], steps_count: 1 },
];

const RUN_OF_CURRENT: RunSummary = {
  run_id: 'run-current',
  workflow: 'owner/demo/lint-sandbox',
  scope: 'sandbox',
  sandbox: 'demo-s1',
  triggered_by: 'ui',
  started_at: '2026-10-18T10:00:00Z',
  status: {
    invocation_id: 'inv-1',
    state: 'Succeeded',
    started_at: '2026-10-18T10:00:00Z',
    ended_at: '2026-10-18T10:01:00Z',
    error: null,
  },
};

function mountPanel() {
  return mount(Workflow, {
    global: {
      provide: { 'sandbox-name': 'demo-s1', 'project-name': 'demo' },
    },
  });
}

function defaultHandler(method: string, url: string): Reply | undefined {
  if (method === 'GET' && url === '/api/projects/demo/workflows') return { body: WORKFLOWS };
  if (method === 'GET' && url.startsWith('/api/sandboxes/demo-s1/runs')) {
    return { body: [RUN_OF_CURRENT] };
  }
  if (method === 'GET' && url.startsWith('/api/projects/demo/runs')) return { body: [] };
  if (method === 'POST' && url === '/api/projects/demo/workflows/1/runs') {
    return { status: 202, body: { run_id: 'run-new', invocation_id: 'inv-2' } };
  }
  return undefined;
}

beforeEach(() => {
  sandboxPhases.value = new Map([['demo-s1', 'Running']]);
  mockApi(defaultHandler);
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

describe('Workflow (panneau)', () => {
  it('panneau_workflows : les deux workflows de portée sandbox sont listés, le lancement cible la sandbox courante', async () => {
    const wrapper = mountPanel();
    await flushPromises();

    expect(wrapper.text()).toContain('lint-sandbox');
    expect(wrapper.text()).toContain('tests-sandbox');
    expect(wrapper.text()).not.toContain('publier-projet');

    const launch = wrapper.findAll('button').find((b) => b.text() === 'Lancer');
    expect(launch).toBeDefined();
    await launch!.trigger('click');
    await flushPromises();

    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain('demo-s1');
    const confirm = Array.from(dialog!.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === 'Lancer',
    );
    confirm!.click();
    await flushPromises();

    const post = calls.find((c) => c.method === 'POST');
    expect(post?.url).toBe('/api/projects/demo/workflows/1/runs');
    expect(post?.body).toEqual({ sandbox: 'demo-s1' });
  });

  it('runs_de_la_sandbox_courante : seuls les runs de la sandbox courante', async () => {
    const wrapper = mountPanel();
    await flushPromises();

    const runCalls = calls.filter((c) => c.url.includes('/runs'));
    expect(runCalls.length).toBeGreaterThan(0);
    expect(runCalls.every((c) => c.url.startsWith('/api/sandboxes/demo-s1/runs'))).toBe(true);
    expect(calls.some((c) => c.url.startsWith('/api/projects/demo/runs'))).toBe(false);
    expect(wrapper.text()).toContain('lint-sandbox');
  });

  it('pas_de_maquette : aucun nœud codé en dur (`fetch-metadata`, `transcode`…) n\'est rendu', async () => {
    const wrapper = mountPanel();
    await flushPromises();

    const text = wrapper.text();
    for (const node of ['fetch-metadata', 'transcode', 'generate-thumbnails', 'extract-subtitles', 'sync-media']) {
      expect(text).not.toContain(node);
    }
  });

  it('panneau_lecture_seule : aucune création, édition ni suppression dans l\'IDE', async () => {
    const wrapper = mountPanel();
    await flushPromises();

    const labels = wrapper.findAll('button').map((b) => b.text());
    expect(labels).not.toContain('Nouveau workflow');
    expect(labels).not.toContain('Éditer');
    expect(labels).not.toContain('Supprimer');
  });

  it('lancement_desactive_hors_running : sandbox non Running, aucun lancement et explication visible', async () => {
    sandboxPhases.value = new Map([['demo-s1', 'Suspended']]);
    const wrapper = mountPanel();
    await flushPromises();

    expect(wrapper.text()).toContain('lancement désactivé');
    const launch = wrapper.findAll('button').find((b) => b.text() === 'Lancer');
    await launch!.trigger('click');
    await flushPromises();

    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(calls.some((c) => c.method === 'POST')).toBe(false);
  });

  it('project_name_reactif : le projet fourni par IdeShell (ref) est suivi par le panneau', async () => {
    const project = ref('demo');
    const wrapper = mount(Workflow, {
      global: { provide: { 'sandbox-name': 'demo-s1', 'project-name': project } },
    });
    await flushPromises();
    expect(wrapper.findComponent(WorkflowList).props('project')).toBe('demo');

    project.value = 'autre';
    await flushPromises();
    expect(wrapper.findComponent(WorkflowList).props('project')).toBe('autre');
  });
});
