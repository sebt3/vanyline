import { createMemoryHistory, createRouter } from 'vue-router';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import type { HealthReport, HealthSummary } from '@vanyline/protocol';
import SandboxOverview from './SandboxOverview.vue';

// Phases de sandbox (WebSocket d'états) : un Map fixé par test, avant le montage.
const { sandboxPhases } = vi.hoisted(() => ({
  sandboxPhases: { value: new Map<string, string>() },
}));

vi.mock('../../composables/useSandboxState', () => ({
  useSandboxState: () => ({ phases: sandboxPhases, connected: { value: true } }),
}));

// ContextChat est en cours d'écriture par un autre agent : stub minimal.
vi.mock('../ContextChat.vue', async () => {
  const { defineComponent, h } = await import('vue');
  return {
    default: defineComponent({
      props: ['kind', 'data'],
      render() {
        return h('div', { 'data-testid': 'context-chat' });
      },
    }),
  };
});

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

function setWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
}

const sandboxFixture = {
  metadata: { name: 's1' },
  spec: { project: 'demo', branch: 'main', suspended: false },
  status: { phase: 'Running' },
};

const workflowsFixture = [
  { id: 3, name: 'Qualité', scope: 'sandbox', kinds: ['gate'], steps_count: 1 },
  { id: 4, name: 'Release', scope: 'project', kinds: ['agent'], steps_count: 1 },
];

type Check = HealthReport['checks'][number];

const check = (id: string, kind: Check['kind'], status: Check['status']): Check => ({
  id,
  kind,
  status,
  exit_code: status === 'passed' ? 0 : null,
  duration_ms: 12,
  output_tail: '',
});

/** Rapport passed : 3 vérifications, couverture 164 / 200 = 82 %. */
function passedReport(): HealthReport {
  return {
    revision: 'abc123',
    checks: [
      check('fmt', 'format', 'passed'),
      check('clippy', 'lint', 'passed'),
      check('test', 'tests', 'passed'),
    ],
    coverage: {
      files: [
        { path: 'src/a.rs', lines_found: 100, lines_hit: 100, uncovered: [] },
        {
          path: 'src/b.rs',
          lines_found: 100,
          lines_hit: 64,
          uncovered: [{ start: 65, end: 100 }],
        },
      ],
    },
    issues: [],
    new_code: null,
    unchecked_languages: [],
  };
}

function passedHealth(phase = 'fresh') {
  // Dérivé par la sandbox : fichiers déjà triés du moins couvert au plus couvert.
  const summary: HealthSummary = {
    status: 'passed',
    coverage_percent: 82,
    files: [
      { path: 'src/b.rs', percent: 64 },
      { path: 'src/a.rs', percent: 100 },
    ],
  };
  return {
    phase,
    computed_at: 1_700_000_000_000,
    stale_since: null,
    base: null,
    report: passedReport(),
    summary,
  };
}

function baseHandler(health: () => unknown): Handler {
  return (method, url) => {
    if (method === 'GET' && url === '/api/sandboxes/s1') return { body: sandboxFixture };
    if (method === 'GET' && url === '/api/sandboxes/s1/code-health') return { body: health() };
    if (method === 'GET' && url === '/api/projects/demo/workflows') return { body: workflowsFixture };
    if (method === 'GET' && url.startsWith('/api/sandboxes/s1/runs')) return { body: [] };
    return undefined;
  };
}

function makeRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', name: 'home', component: { render: () => null } },
      {
        path: '/p/:projectName/s/:sandboxName/overview',
        name: 'sandbox-overview',
        component: SandboxOverview,
        props: true,
      },
      { path: '/p/:projectName/s/:sandboxName', name: 'ide', component: { render: () => null } },
      { path: '/p/:projectName/runs/:runId', name: 'run', component: { render: () => null } },
    ],
  });
}

let wrapper: VueWrapper | undefined;

async function mountOverview(width = 1280) {
  setWidth(width);
  const router = makeRouter();
  await router.push('/p/demo/s/s1/overview');
  await router.isReady();
  wrapper = mount(SandboxOverview, {
    props: { projectName: 'demo', sandboxName: 's1' },
    global: { plugins: [router] },
    attachTo: document.body,
  });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  vi.restoreAllMocks();
  sandboxPhases.value = new Map([['s1', 'Running']]);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
  setWidth(1024);
});

describe('SandboxOverview', () => {
  it('sante_complete', async () => {
    mockApi(baseHandler(() => passedHealth()));
    const w = await mountOverview();
    expect(w.text()).toContain('82 %');
    expect(w.find('[data-testid="verdict"]').text()).toContain('passed');
    expect(w.findAll('[data-testid="check-row"]')).toHaveLength(3);
    expect(w.find('[data-testid="computed-at"]').exists()).toBe(true);
  });

  it('rapport_perime', async () => {
    mockApi(baseHandler(() => passedHealth('stale')));
    const w = await mountOverview();
    expect(w.find('[data-testid="freshness"]').text()).toContain('Périmé');
  });

  it('calcul_en_cours', async () => {
    mockApi(baseHandler(() => passedHealth('computing')));
    const w = await mountOverview();
    expect(w.find('[data-testid="freshness"]').text()).toContain('Calcul en cours');
    expect(w.findAll('[data-testid="check-row"]')).toHaveLength(3);
  });

  it('premier_calcul', async () => {
    mockApi(baseHandler(() => ({ phase: 'computing', report: null, summary: null, computed_at: null })));
    const w = await mountOverview();
    expect(w.find('[data-testid="health-loading"]').exists()).toBe(true);
    expect(w.findAll('[data-testid="check-row"]')).toHaveLength(0);
    expect(w.find('[data-testid="coverage-total"]').exists()).toBe(false);
  });

  it('verification_en_erreur', async () => {
    const health = passedHealth();
    health.report.checks[1] = check('clippy', 'lint', 'errored');
    health.summary.status = 'errored';
    mockApi(baseHandler(() => health));
    const w = await mountOverview();
    const warning = w.find('[data-testid="errored-warning"]');
    expect(warning.exists()).toBe(true);
    expect(warning.text()).toContain('clippy');
    const verdict = w.find('[data-testid="verdict"]');
    expect(verdict.text()).toContain('errored');
    expect(verdict.classes()).not.toContain('verdict--ok');
  });

  it('statut_failed_affiche', async () => {
    const health = passedHealth();
    health.report.checks[2] = check('test', 'tests', 'failed');
    health.summary.status = 'failed';
    mockApi(baseHandler(() => health));
    const w = await mountOverview();
    const verdict = w.find('[data-testid="verdict"]');
    expect(verdict.text()).toContain('failed');
    expect(verdict.text()).toContain('Échec');
    expect(verdict.classes()).toContain('verdict--fail');
    expect(verdict.classes()).not.toContain('verdict--ok');
    expect(w.find('[data-testid="checks-summary"]').text()).toContain('1 échec(s)');
  });

  it('erreur_de_sondage_rapport_perime', async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    let failing = false;
    const base = baseHandler(() => passedHealth('fresh'));
    mockApi((method, url, body) =>
      method === 'GET' && url === '/api/sandboxes/s1/code-health'
        ? failing
          ? { status: 500, body: { error: 'boom' } }
          : { body: passedHealth('fresh') }
        : base(method, url, body),
    );
    const w = await mountOverview();
    expect(w.find('[data-testid="freshness"]').text()).toContain('À jour');
    expect(w.find('[data-testid="poll-error"]').exists()).toBe(false);

    // Le sondage suivant échoue : le dernier rapport reste affiché, mais pas comme à jour.
    failing = true;
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(w.findAll('[data-testid="check-row"]')).toHaveLength(3);
    const warning = w.find('[data-testid="poll-error"]');
    expect(warning.exists()).toBe(true);
    expect(warning.text()).toContain('possiblement périmées');
    expect(w.find('[data-testid="freshness"]').text()).not.toContain('À jour');

    Reflect.deleteProperty(document, 'visibilityState');
  });

  it('langages_non_verifies', async () => {
    const health = passedHealth();
    health.report.unchecked_languages = ['go'];
    mockApi(baseHandler(() => health));
    const w = await mountOverview();
    expect(w.find('[data-testid="unchecked-warning"]').text()).toContain('go non vérifié');
  });

  it('recalcul_force', async () => {
    let refreshed = false;
    mockApi((method, url, body) => {
      if (method === 'POST' && url === '/api/sandboxes/s1/code-health/refresh') {
        refreshed = true;
        return { status: 202, body: {} };
      }
      return baseHandler(() => passedHealth(refreshed ? 'computing' : 'fresh'))(method, url, body);
    });
    const w = await mountOverview();
    const button = w.findAll('button').find((b) => b.text().includes('Recalculer'));
    expect(button).toBeDefined();
    await button!.trigger('click');
    await flushPromises();
    expect(calls.some((c) => c.method === 'POST' && c.url === '/api/sandboxes/s1/code-health/refresh')).toBe(true);
    expect(w.find('[data-testid="freshness"]').text()).toContain('Calcul en cours');
  });

  it('lancement_sur_cette_sandbox', async () => {
    mockApi((method, url, body) => {
      if (method === 'POST' && url === '/api/projects/demo/workflows/3/runs') {
        return { status: 202, body: { run_id: 'r1', invocation_id: 'i1' } };
      }
      return baseHandler(() => passedHealth())(method, url, body);
    });
    const w = await mountOverview();
    expect(w.find('[data-testid="launch-4"]').exists()).toBe(false);
    await w.find('[data-testid="launch-3"]').trigger('click');
    await flushPromises();
    const confirm = [...document.querySelectorAll('[role="dialog"] button')].find(
      (b) => b.textContent?.trim() === 'Lancer',
    ) as HTMLButtonElement | undefined;
    expect(confirm).toBeDefined();
    confirm!.click();
    await flushPromises();
    const launch = calls.find((c) => c.method === 'POST' && c.url.endsWith('/workflows/3/runs'));
    expect(launch?.body).toEqual({ sandbox: 's1' });
  });

  it('runs_de_la_sandbox', async () => {
    mockApi(baseHandler(() => passedHealth()));
    await mountOverview();
    const runCalls = calls.filter((c) => c.url.includes('/runs'));
    expect(runCalls.length).toBeGreaterThan(0);
    expect(runCalls.every((c) => c.url.startsWith('/api/sandboxes/s1/runs'))).toBe(true);
  });

  it('mise_en_page_360px', async () => {
    const health = passedHealth();
    health.report.issues = [
      {
        check_id: 'clippy',
        path: 'very/long/nested/directory/structure/that/goes/on/and/on/file.rs',
        line: 4,
        column: 1,
        severity: 'warning',
        code: null,
        message: 'message',
      },
    ];
    mockApi(baseHandler(() => health));
    const w = await mountOverview(360);
    expect(w.findAll('table')).toHaveLength(0);
    const details = w.findAll('details');
    expect(details.length).toBeGreaterThan(0);
    const path = w.find('[data-testid="issue-path"]');
    expect(path.text()).toContain('file.rs');
    expect(path.text().length).toBeLessThan(
      'very/long/nested/directory/structure/that/goes/on/and/on/file.rs'.length,
    );
  });

  it('lien_ide_selon_la_largeur', async () => {
    mockApi(baseHandler(() => passedHealth()));
    const w = await mountOverview(800);
    expect(w.find('[data-testid="ide-disabled"]').text()).toMatch(/1024/);
    expect(w.find('[data-testid="ide-link"]').exists()).toBe(false);
    setWidth(1280);
    window.dispatchEvent(new Event('resize'));
    await nextTick();
    expect(w.find('[data-testid="ide-link"]').exists()).toBe(true);
    expect(w.find('[data-testid="ide-disabled"]').exists()).toBe(false);
  });

  it('pas_de_mesure_jamais_zero', async () => {
    const health = passedHealth();
    health.report.coverage = null;
    health.summary.coverage_percent = null;
    health.summary.files = [];
    mockApi(baseHandler(() => health));
    const w = await mountOverview();
    expect(w.find('[data-testid="coverage-missing"]').exists()).toBe(true);
    expect(w.find('[data-testid="coverage-total"]').exists()).toBe(false);
    expect(w.text()).not.toContain('0 %');
  });

  it('polling_10s_30s_arret_masque', async () => {
    let visibility = 'visible';
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => visibility,
    });
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    let phase = 'computing';
    mockApi(baseHandler(() => passedHealth(phase)));
    await mountOverview();
    const healthCalls = () =>
      calls.filter((c) => c.method === 'GET' && c.url === '/api/sandboxes/s1/code-health').length;

    expect(healthCalls()).toBe(1);
    // Calcul en cours : interrogation toutes les 10 s.
    await vi.advanceTimersByTimeAsync(9_999);
    await flushPromises();
    expect(healthCalls()).toBe(1);
    phase = 'fresh';
    await vi.advanceTimersByTimeAsync(1);
    await flushPromises();
    expect(healthCalls()).toBe(2);
    // Rapport frais : interrogation toutes les 30 s.
    await vi.advanceTimersByTimeAsync(29_999);
    await flushPromises();
    expect(healthCalls()).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    await flushPromises();
    expect(healthCalls()).toBe(3);
    // Onglet masqué : arrêt.
    visibility = 'hidden';
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(120_000);
    await flushPromises();
    expect(healthCalls()).toBe(3);
    // Onglet de nouveau visible : rechargement immédiat.
    visibility = 'visible';
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(healthCalls()).toBe(4);

    vi.useRealTimers();
    Reflect.deleteProperty(document, 'visibilityState');
  });

  it('aucun_appel_direct', async () => {
    mockApi(baseHandler(() => passedHealth()));
    await mountOverview();
    expect(calls.length).toBeGreaterThan(0);
    expect(calls.every((c) => c.url.startsWith('/api/'))).toBe(true);
  });
});
