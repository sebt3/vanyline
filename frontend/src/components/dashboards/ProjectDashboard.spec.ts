import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ProjectDashboard from './ProjectDashboard.vue';
import ContextChat from '../ContextChat.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
});

function createTestRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', name: 'home', component: { template: '<div>Home</div>' } },
      {
        path: '/p/:projectName',
        name: 'project',
        component: ProjectDashboard,
        props: true,
      },
      {
        path: '/p/:projectName/s/:sandboxName',
        name: 'ide',
        component: { template: '<div>IdeShell</div>' },
      },
      { path: '/settings', name: 'settings', component: { template: '<div>Settings</div>' } },
    ],
  });
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status < 400,
    status,
    headers: new Map([['content-type', 'application/json']]),
    json: () => Promise.resolve(body),
  } as unknown as Response;
}

/** Mock GET /api/projects/:name — `cloned: true` par défaut (projet prêt),
 *  pour ne pas avoir à s'en soucier dans chaque test qui ne teste pas
 *  spécifiquement ce garde-fou. */
function mockProjectFetch(name: string, cloned = true) {
  return { url: `/api/projects/${name}`, method: 'GET', body: { metadata: { name }, status: { cloned } } };
}

/** Route `fetch` par (méthode, URL exacte ou testée avec `.includes`) parmi
 *  `routes`, dans l'ordre — chaque route peut être appelée plusieurs fois
 *  (les callers ré-invoquent souvent la même route après une mutation). */
function routeFetch(
  fetchSpy: ReturnType<typeof vi.spyOn>,
  routes: Array<{ url: string; method: string; body: unknown; status?: number; once?: boolean }>,
) {
  const used = new Set<number>();
  (fetchSpy as any).mockImplementation(async (url: string, init?: RequestInit) => {
    const m = (init?.method ?? 'GET') as string;
    const u = String(url);
    for (let i = 0; i < routes.length; i++) {
      const r = routes[i];
      if (r.once && used.has(i)) continue;
      if (r.method === m && u === r.url) {
        used.add(i);
        return jsonResponse(r.body, r.status ?? 200);
      }
    }
    return new Response(null, { status: 500 });
  });
}

describe('ProjectDashboard', () => {
  it('filtre les sandboxes : GET renvoie 3 sandboxes (2 foo, 1 bar) → rendu ne contient que foo', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo'),
      {
        url: '/api/sandboxes',
        method: 'GET',
        body: [
          {
            metadata: { name: 'sb-alpha' },
            spec: { project: 'foo', branch: 'main', suspended: false },
            status: { phase: 'Running' },
          },
          {
            metadata: { name: 'sb-beta' },
            spec: {
              project: 'foo',
              branch: 'develop',
              toolchains: [{ name: 'rust', image: 'rust:slim' }],
            },
            status: { phase: 'Pending' },
          },
          {
            metadata: { name: 'sb-gamma' },
            spec: { project: 'bar', branch: 'main', suspended: false },
            status: { phase: 'Running' },
          },
        ],
      },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('sb-alpha');
    expect(wrapper.text()).toContain('sb-beta');
    expect(wrapper.text()).not.toContain('sb-gamma');
  });

  it('cliquer une ligne navigue vers /p/<project>/s/<sandbox>', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo'),
      {
        url: '/api/sandboxes',
        method: 'GET',
        body: [
          {
            metadata: { name: 'sb-alpha' },
            spec: { project: 'foo', branch: 'main', suspended: false },
            status: { phase: 'Running' },
          },
        ],
      },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const row = wrapper.find('tr.row-clickable');
    await row.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/p/foo/s/sb-alpha');
  });

  it('sandbox pas Running → clic ligne/bouton Ouvrir sans effet, bouton désactivé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo'),
      {
        url: '/api/sandboxes',
        method: 'GET',
        body: [
          {
            metadata: { name: 'sb-provisioning' },
            spec: { project: 'foo', branch: 'main', suspended: false },
            status: { phase: 'Provisioning' },
          },
        ],
      },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const openBtn = wrapper.find('.btn-open');
    expect(openBtn.attributes('disabled')).toBeDefined();
    expect(wrapper.find('tr.row-clickable').exists()).toBe(false);

    await wrapper.find('tbody tr').trigger('click');
    await openBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/p/foo');
  });

  it('cliquer "Supprimer" → DELETE /api/sandboxes/sb-to-del puis re-fetch → "Aucune sandbox"', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let sandboxFetchCount = 0;
    (fetchSpy as any).mockImplementation(async (url: string, init?: RequestInit) => {
      const m = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (m === 'GET' && u === '/api/projects/foo') {
        return jsonResponse({ metadata: { name: 'foo' }, status: { cloned: true } });
      }
      if (m === 'DELETE' && u.includes('/api/sandboxes/sb-to-del')) {
        return new Response(null, { status: 204 });
      }
      if (m === 'GET' && u === '/api/sandboxes') {
        sandboxFetchCount++;
        if (sandboxFetchCount === 1) {
          return jsonResponse([
            {
              metadata: { name: 'sb-to-del' },
              spec: { project: 'foo', branch: 'b', suspended: false },
              status: { phase: 'Running' },
            },
          ]);
        }
        return jsonResponse([]);
      }
      return new Response(null, { status: 500 });
    });

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/sandboxes/sb-to-del'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({ method: 'DELETE' });

    expect(wrapper.text()).toContain('Aucune sandbox');
    expect(router.currentRoute.value.path).toBe('/p/foo');
  });

  it('cliquer "Suspendre" → POST avec suspended: true puis re-fetch → libellé devient "Reprendre"', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    // Drapeau posé par le POST de suspension : ne dépend pas du nombre total
    // d'appels (les sections workflows/runs en ajoutent).
    let suspendedNow = false;
    (fetchSpy as any).mockImplementation(async (url: string, init?: RequestInit) => {
      const m = init?.method as string;
      const u = String(url);

      if (m === 'GET' && u === '/api/projects/foo') {
        return jsonResponse({ metadata: { name: 'foo' }, status: { cloned: true } });
      }

      if (m === 'POST' && u.includes('/api/sandboxes/sb-to-suspend/suspend')) {
        suspendedNow = true;
        return jsonResponse({
          metadata: { name: 'sb-to-suspend' },
          spec: { project: 'foo', branch: 'b', suspended: true },
          status: { phase: 'Suspended' },
        });
      }

      if (m === 'GET' && u === '/api/sandboxes') {
        if (!suspendedNow) {
          return jsonResponse([
            {
              metadata: { name: 'sb-to-suspend' },
              spec: { project: 'foo', branch: 'b', suspended: false },
              status: { phase: 'Running' },
            },
          ]);
        }
        return jsonResponse([
          {
            metadata: { name: 'sb-to-suspend' },
            spec: { project: 'foo', branch: 'b', suspended: true },
            status: { phase: 'Suspended' },
          },
        ]);
      }

      return new Response(null, { status: 500 });
    });

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Suspendre');

    const suspendBtn = wrapper.find('.btn-suspend');
    await suspendBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Reprendre');
    expect(router.currentRoute.value.path).toBe('/p/foo');
  });

  it('cliquer Retour → / ; cliquer Paramètres → /settings', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo'),
      { url: '/api/sandboxes', method: 'GET', body: [] },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Retour
    const backBtn = wrapper.find('.btn-back');
    await backBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));
    expect(router.currentRoute.value.path).toBe('/');

    // Paramètres
    const settingsBtn = wrapper.find('.btn-settings');
    await settingsBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));
    expect(router.currentRoute.value.path).toBe('/settings');
  });

  it('création en modale : dialog apparaît → remplir Nom + Branche → POST avec project: projectName → dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    (fetchSpy as any).mockImplementation(async (url: string, init?: RequestInit) => {
      const u = String(url);
      const m = init?.method as string;

      if (m === 'GET' && u === '/api/projects/foo') {
        return jsonResponse({ metadata: { name: 'foo' }, status: { cloned: true } });
      }
      if (m === 'GET' && u === '/api/sandboxes') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse([]);
        }
        return jsonResponse([
          {
            metadata: { name: 'new-sb' },
            spec: { project: 'foo', branch: 'develop', suspended: false },
            status: { phase: 'Pending' },
          },
        ]);
      }
      if (m === 'POST' && u === '/api/sandboxes') {
        const body = JSON.parse(((init as RequestInit)?.body as string) ?? '{}');
        expect(body).toEqual({
          name: 'ma-sandbox',
          project: 'foo',
          branch: 'develop',
        });
        return jsonResponse(
          {
            metadata: { name: 'ma-sandbox' },
            spec: { project: 'foo', branch: 'develop', suspended: false },
            status: { phase: 'Pending' },
          },
          201,
        );
      }
      return new Response(null, { status: 500 });
    });

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Créer une sandbox" → dialog apparaît
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="dialog"]')).toBeTruthy();

    const dialog = document.querySelector('[role="dialog"]')!;

    // Remplir les champs
    const inputs = dialog.querySelectorAll('input');
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(inputs[0], 'ma-sandbox');
    setInput(inputs[1], 'develop');

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create');
    await (dialogCreateBtn as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Re-fetch → new-sb apparaît
    expect(wrapper.text()).toContain('new-sb');

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();
  });

  it('GET sandboxes en erreur → message d\'erreur affiché, pas de tableau', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo'),
      {
        url: '/api/sandboxes',
        method: 'GET',
        body: { error: 'VNL-AUTH-001: Non autorisé' },
        status: 401,
      },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('VNL-AUTH-001');
    expect(wrapper.text()).not.toContain('sb-alpha');
  });

  it('projet pas encore cloné → bouton "Créer une sandbox" désactivé, message affiché', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo', false),
      { url: '/api/sandboxes', method: 'GET', body: [] },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const createBtn = wrapper.find('.btn-create');
    expect(createBtn.attributes('disabled')).toBeDefined();
    expect(wrapper.text()).toContain('Clone du projet en cours');

    // Cliquer sur le bouton désactivé ne doit pas ouvrir le dialog
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();
  });

  it('projet cloné → bouton "Créer une sandbox" actif, pas de message', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    routeFetch(fetchSpy, [
      mockProjectFetch('foo', true),
      { url: '/api/sandboxes', method: 'GET', body: [] },
    ]);

    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();

    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router] },
      props: { projectName: 'foo' },
    });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const createBtn = wrapper.find('.btn-create');
    expect(createBtn.attributes('disabled')).toBeUndefined();
    expect(wrapper.text()).not.toContain('Clone du projet en cours');
  });
});

// ---------------------------------------------------------------------------
// dashboards.sdd — section WORKFLOWS, section RUNS, actions par sandbox,
// assistant de projet (ContextChat stubé), moteur non configuré.
// ---------------------------------------------------------------------------

type Route = { status?: number; body?: unknown };

function setWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
}

async function flush() {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

/** Fetch par clé `MÉTHODE /url?query`, réponse 500 si la clé est absente. */
function fetchByKey(spy: ReturnType<typeof vi.spyOn>, table: Record<string, Route>) {
  (spy as any).mockImplementation(async (url: string, init?: RequestInit) => {
    const key = `${init?.method ?? 'GET'} ${String(url)}`;
    const entry = table[key];
    if (!entry) return new Response(null, { status: 500 });
    return jsonResponse(entry.body ?? null, entry.status ?? 200);
  });
}

function projectTable(p: string): Record<string, Route> {
  return {
    [`GET /api/projects/${p}`]: { body: { metadata: { name: p }, status: { cloned: true } } },
    'GET /api/sandboxes': {
      body: [
        { metadata: { name: 'sb-run' }, spec: { project: p, branch: 'main' }, status: { phase: 'Running' } },
        { metadata: { name: 'sb-stop' }, spec: { project: p, branch: 'main' }, status: { phase: 'Suspended' } },
      ],
    },
    [`GET /api/projects/${p}/runs?limit=50&since=14`]: { body: [] },
  };
}

const WF_PROJECT = { id: 1, name: 'build', scope: 'project', kinds: ['agent'], steps_count: 2 };
const WF_SANDBOX = { id: 3, name: 'gate-run', scope: 'sandbox', kinds: ['gate'], steps_count: 1 };

function runRow(id: string, workflow: string, state: string) {
  return {
    run_id: id,
    workflow,
    scope: 'project',
    triggered_by: 'ui',
    started_at: '2026-10-18T10:00:00Z',
    status: { invocation_id: `inv-${id}`, state, started_at: '2026-10-18T10:00:00Z', ended_at: null, error: null },
  };
}

async function mountProject(p: string, table: Record<string, Route>, width = 1024) {
  setWidth(width);
  const spy = vi.spyOn(globalThis, 'fetch');
  fetchByKey(spy, table);
  const router = createTestRouter();
  await router.push(`/p/${p}`);
  await router.isReady();
  const wrapper = mount(ProjectDashboard, {
    global: { plugins: [router], stubs: { ContextChat: true } },
    props: { projectName: p },
  });
  await flush();
  return { wrapper, router, spy };
}

function dialogButton(text: string): HTMLButtonElement | undefined {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')).find(
    (b) => b.textContent?.trim() === text,
  );
}

describe('ProjectDashboard — dashboards.sdd (G2a)', () => {
  afterEach(() => {
    setWidth(1024);
  });

  it('projet_liste_workflows_et_runs', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_PROJECT, WF_SANDBOX] },
      'GET /api/projects/foo/runs?limit=50&since=14': {
        body: [
          runRow('r-new', 'o/foo/run-new', 'Succeeded'),
          runRow('r-mid', 'o/foo/run-mid', 'Running'),
          runRow('r-old', 'o/foo/run-old', 'Failed'),
        ],
      },
    };
    const { wrapper } = await mountProject('foo', table);
    const text = wrapper.text();
    expect(text).toContain('build');
    expect(text).toContain('gate-run');
    expect(text.indexOf('run-new')).toBeGreaterThan(-1);
    expect(text.indexOf('run-new')).toBeLessThan(text.indexOf('run-mid'));
    expect(text.indexOf('run-mid')).toBeLessThan(text.indexOf('run-old'));
  });

  it('lancement_workflow_de_sandbox', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_SANDBOX] },
      'POST /api/projects/foo/workflows/3/runs': { body: { run_id: 'r1', invocation_id: 'i1' } },
    };
    const { wrapper, spy } = await mountProject('foo', table);
    await wrapper.find('.btn-launch').trigger('click');
    await flush();
    const options = Array.from(document.querySelectorAll<HTMLOptionElement>('select[aria-label="Sandbox"] option'));
    expect(options.map((o) => o.value)).toEqual(['sb-run']);
    dialogButton('Lancer')?.click();
    await flush();
    const post = spy.mock.calls.find(
      ([u, init]) => u === '/api/projects/foo/workflows/3/runs' && (init as RequestInit)?.method === 'POST',
    );
    expect(post).toBeDefined();
    expect(JSON.parse((post![1] as RequestInit).body as string)).toEqual({ sandbox: 'sb-run' });
  });

  it('lancement_workflow_de_projet', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_PROJECT] },
      'POST /api/projects/foo/workflows/1/runs': { body: { run_id: 'r2', invocation_id: 'i2' } },
    };
    const { wrapper, spy } = await mountProject('foo', table);
    await wrapper.find('.btn-launch').trigger('click');
    await flush();
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain('lancement au niveau du projet');
    dialogButton('Lancer')?.click();
    await flush();
    const post = spy.mock.calls.find(
      ([u, init]) => u === '/api/projects/foo/workflows/1/runs' && (init as RequestInit)?.method === 'POST',
    );
    expect(post).toBeDefined();
    expect(JSON.parse((post![1] as RequestInit).body as string)).toEqual({});
  });

  it('aucune_sandbox_demarree', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/sandboxes': {
        body: [{ metadata: { name: 'sb-stop' }, spec: { project: 'foo', branch: 'main' }, status: { phase: 'Suspended' } }],
      },
      'GET /api/projects/foo/workflows': { body: [WF_SANDBOX] },
    };
    const { wrapper } = await mountProject('foo', table);
    await wrapper.find('.btn-launch').trigger('click');
    await flush();
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
      "Aucune sandbox en cours d'exécution",
    );
    expect(dialogButton('Lancer')?.disabled).toBe(true);
  });

  it('moteur_non_configure', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { status: 503, body: { code: 'VNL-WF-503', message: 'engine' } },
      'GET /api/projects/foo/runs?limit=50&since=14': { status: 503, body: { code: 'VNL-WF-503', message: 'engine' } },
    };
    const { wrapper } = await mountProject('foo', table);
    // Message dédié du moteur (WorkflowList / RunList) : « indisponible » dans l'état livré.
    expect(wrapper.text()).toContain('Moteur de workflow indisponible');
    // Le reste de la page fonctionne toujours.
    expect(wrapper.text()).toContain('sb-run');
  });

  it('deux_actions_d_ouverture_par_sandbox', async () => {
    const wide = await mountProject('foo', projectTable('foo'), 1280);
    const ideWide = wide.wrapper.find('.btn-open');
    expect(ideWide.text()).toBe('IDE');
    expect(ideWide.attributes('disabled')).toBeUndefined();
    const overviewWide = wide.wrapper.find('.btn-overview');
    expect(overviewWide.text()).toBe("Vue d'ensemble");
    expect(overviewWide.attributes('disabled')).toBeUndefined();
    wide.wrapper.unmount();

    const narrow = await mountProject('foo', projectTable('foo'), 800);
    expect(narrow.wrapper.find('.btn-open').attributes('disabled')).toBeDefined();
    expect(narrow.wrapper.text()).toContain('1024 px');
    expect(narrow.wrapper.find('.btn-overview').attributes('disabled')).toBeUndefined();
  });

  it('assistant_projet', async () => {
    const { wrapper } = await mountProject('demo', projectTable('demo'), 360);
    expect(wrapper.findComponent(ContextChat).exists()).toBe(false);
    await wrapper.find('.btn-assistant').trigger('click');
    await flush();
    const chat = wrapper.findComponent(ContextChat);
    expect(chat.exists()).toBe(true);
    expect(chat.props('kind')).toBe('project');
    expect(chat.props('data')).toEqual({ project_name: 'demo' });
    expect(wrapper.find('[data-layout="sheet"]').exists()).toBe(true);
  });

  it('mise_en_page_mobile', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_PROJECT] },
    };
    const { wrapper } = await mountProject('foo', table, 360);
    const sections = wrapper.findAll('[data-section]').map((s) => s.attributes('data-section'));
    expect(sections).toEqual(['sandboxes', 'workflows', 'runs']);
    await wrapper.find('.btn-assistant').trigger('click');
    await flush();
    expect(wrapper.find('[data-layout="sheet"]').exists()).toBe(true);
  });
});

describe('ProjectDashboard — identifiants encodés dans les URL', () => {
  afterEach(() => {
    setWidth(1024);
  });

  it('nom de projet et de sandbox réservés : URL API et navigation encodées', async () => {
    // Projet `a?b`, sandbox `sb c` : le segment d'URL est `a%3Fb` / `sb%20c`, jamais brut.
    const table = {
      ...projectTable('a%3Fb'),
      'GET /api/sandboxes': {
        body: [
          { metadata: { name: 'sb c' }, spec: { project: 'a?b', branch: 'main' }, status: { phase: 'Running' } },
        ],
      },
    };
    setWidth(1280);
    const spy = vi.spyOn(globalThis, 'fetch');
    fetchByKey(spy, table);
    const router = createTestRouter();
    await router.push('/p/a%3Fb');
    await router.isReady();
    const wrapper = mount(ProjectDashboard, {
      global: { plugins: [router], stubs: { ContextChat: true } },
      props: { projectName: 'a?b' },
    });
    await flush();

    const urls = spy.mock.calls.map(([u]) => String(u));
    expect(urls).toContain('/api/projects/a%3Fb');
    expect(urls.some((u) => u.includes('a?b'))).toBe(false);

    await wrapper.find('tr.row-clickable').trigger('click');
    await flush();
    expect(router.currentRoute.value.path).toBe('/p/a%3Fb/s/sb%20c');
    expect(router.currentRoute.value.params.projectName).toBe('a?b');
  });
});

// ---------------------------------------------------------------------------
// Flux de workflow depuis l'écran projet : création, édition, erreurs (422, 503).
// ---------------------------------------------------------------------------

function setField(el: Element | null, value: string) {
  const input = el as HTMLInputElement;
  input.value = value;
  input.dispatchEvent(new Event('input'));
}

function openNewWorkflowEditor(wrapper: { find: (s: string) => { trigger: (e: string) => Promise<unknown> } }) {
  return wrapper.find('[data-section="workflows"] .btn-primary').trigger('click');
}

const DIALOG_WORKFLOW_SAVE = '[role="dialog"] [data-testid="save"]';

describe('ProjectDashboard — flux de workflow (création, édition, erreurs)', () => {
  afterEach(() => {
    setWidth(1024);
    document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
  });

  it('creation_workflow_envoie_le_corps_de_l_editeur', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [] },
      'POST /api/projects/foo/workflows': { status: 201, body: null },
    };
    const { wrapper, spy } = await mountProject('foo', table);
    await openNewWorkflowEditor(wrapper);
    await flush();

    setField(document.querySelector('[role="dialog"] [data-testid="name"]'), 'nouveau');
    document.querySelector<HTMLButtonElement>('[role="dialog"] [data-testid="add-step"]')?.click();
    await flush();
    setField(document.querySelector('[role="dialog"] [data-testid="step-id-0"]'), 's1');
    setField(document.querySelector('[role="dialog"] [data-testid="agent-name-0"]'), 'claude');
    document.querySelector<HTMLButtonElement>(DIALOG_WORKFLOW_SAVE)?.click();
    await flush();

    const post = spy.mock.calls.find(
      ([u, init]) => u === '/api/projects/foo/workflows' && (init as RequestInit)?.method === 'POST',
    );
    expect(post).toBeDefined();
    expect(JSON.parse((post![1] as RequestInit).body as string)).toEqual({
      name: 'nouveau',
      scope: 'project',
      steps: [{ id: 's1', kind: 'agent', depends_on: [], config: { agent: 'claude', prompt: '' } }],
    });
    // Succès : dialogue fermé.
    expect(document.querySelector('[role="dialog"] [data-testid="save"]')).toBeNull();
  });

  it('edition_workflow_envoie_put_sur_son_id', async () => {
    const existing = {
      ...WF_PROJECT,
      description: 'avant',
      steps: [{ id: 'b1', kind: 'agent', depends_on: [], config: { agent: 'claude', prompt: 'hi' } }],
    };
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_PROJECT] },
      'GET /api/projects/foo/workflows/1': { body: existing },
      'PUT /api/projects/foo/workflows/1': { status: 200, body: null },
    };
    const { wrapper, spy } = await mountProject('foo', table);
    const editBtn = wrapper.findAll('[data-section="workflows"] button').find((b) => b.text() === 'Éditer');
    expect(editBtn).toBeDefined();
    await editBtn!.trigger('click');
    await flush();

    expect(document.querySelector<HTMLInputElement>('[role="dialog"] [data-testid="name"]')?.disabled).toBe(true);
    setField(document.querySelector('[role="dialog"] [data-testid="description"]'), 'après');
    document.querySelector<HTMLButtonElement>(DIALOG_WORKFLOW_SAVE)?.click();
    await flush();

    const put = spy.mock.calls.find(
      ([u, init]) => u === '/api/projects/foo/workflows/1' && (init as RequestInit)?.method === 'PUT',
    );
    expect(put).toBeDefined();
    expect(JSON.parse((put![1] as RequestInit).body as string)).toEqual({
      description: 'après',
      steps: [{ id: 'b1', kind: 'agent', depends_on: [], config: { agent: 'claude', prompt: 'hi' } }],
    });
    expect(document.querySelector('[role="dialog"] [data-testid="save"]')).toBeNull();
  });

  it('erreur_422_affichee_sur_le_step_dialogue_reste_ouvert_sans_perte', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [] },
      'POST /api/projects/foo/workflows': {
        status: 422,
        body: { code: 'VNL-WF-422', error: 'step `s1` : kind shell interdit pour cette portée' },
      },
    };
    const { wrapper } = await mountProject('foo', table);
    await openNewWorkflowEditor(wrapper);
    await flush();

    setField(document.querySelector('[role="dialog"] [data-testid="name"]'), 'nouveau');
    document.querySelector<HTMLButtonElement>('[role="dialog"] [data-testid="add-step"]')?.click();
    await flush();
    setField(document.querySelector('[role="dialog"] [data-testid="step-id-0"]'), 's1');
    setField(document.querySelector('[role="dialog"] [data-testid="agent-name-0"]'), 'claude');
    document.querySelector<HTMLButtonElement>(DIALOG_WORKFLOW_SAVE)?.click();
    await flush();

    // Le dialogue reste ouvert, le message est sur le step cité, rien n'est perdu.
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(document.querySelector('[role="dialog"] [data-testid="step-error"]')?.textContent).toContain(
      'kind shell interdit pour cette portée',
    );
    expect(document.querySelector<HTMLInputElement>('[role="dialog"] [data-testid="name"]')?.value).toBe('nouveau');
    expect(document.querySelector<HTMLInputElement>('[role="dialog"] [data-testid="step-id-0"]')?.value).toBe('s1');
    expect(document.querySelector<HTMLInputElement>('[role="dialog"] [data-testid="agent-name-0"]')?.value).toBe(
      'claude',
    );
    // La page reste utilisable.
    expect(wrapper.find('[data-section="workflows"]').exists()).toBe(true);
  });

  it('erreur_503_moteur_indisponible_sans_casser_la_page', async () => {
    const table = {
      ...projectTable('foo'),
      'GET /api/projects/foo/workflows': { body: [WF_PROJECT] },
      'POST /api/projects/foo/workflows': { status: 503, body: { code: 'VNL-WF-503', error: 'engine' } },
    };
    const { wrapper } = await mountProject('foo', table);
    await openNewWorkflowEditor(wrapper);
    await flush();

    setField(document.querySelector('[role="dialog"] [data-testid="name"]'), 'nouveau');
    await flush();
    document.querySelector<HTMLButtonElement>(DIALOG_WORKFLOW_SAVE)?.click();
    await flush();

    const banner = document.querySelector('[role="dialog"] [data-testid="editor-error"]');
    expect(banner?.textContent).toContain('Moteur de workflow indisponible');
    expect(document.querySelector<HTMLInputElement>('[role="dialog"] [data-testid="name"]')?.value).toBe('nouveau');
    // Le reste de la page fonctionne toujours.
    expect(wrapper.text()).toContain('build');
    expect(wrapper.find('[data-section="sandboxes"]').exists()).toBe(true);
  });
});
