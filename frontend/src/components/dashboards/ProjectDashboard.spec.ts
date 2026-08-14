import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import ProjectDashboard from './ProjectDashboard.vue';

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
    let fetchCount = 0;
    (fetchSpy as any).mockImplementation(async (url: string, init?: RequestInit) => {
      fetchCount++;
      const m = init?.method as string;
      const u = String(url);

      if (m === 'GET' && u === '/api/projects/foo') {
        return jsonResponse({ metadata: { name: 'foo' }, status: { cloned: true } });
      }

      if (m === 'POST' && u.includes('/api/sandboxes/sb-to-suspend/suspend')) {
        return jsonResponse({
          metadata: { name: 'sb-to-suspend' },
          spec: { project: 'foo', branch: 'b', suspended: true },
          status: { phase: 'Suspended' },
        });
      }

      if (m === 'GET' && u === '/api/sandboxes') {
        if (fetchCount <= 2) {
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
