import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import HomeDashboard from './HomeDashboard.vue';

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
      { path: '/', name: 'home', component: HomeDashboard },
      { path: '/p/:projectName', name: 'project', component: { template: '<div>Project</div>' } },
      {
        path: '/p/:projectName/s/:sandboxName',
        name: 'ide',
        component: { template: '<div>IdeShell</div>' },
      },
      { path: '/settings', name: 'settings', component: { template: '<div>Settings</div>' } },
    ],
  });
}

describe('HomeDashboard', () => {
  it('affiche les 2 noms + repoUrl quand GET /api/projects renvoie 2 projets', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            metadata: { name: 'proj-alpha' },
            spec: { owner: 'u1', repoUrl: 'https://github.com/org/alpha', defaultBranch: 'main' },
          },
          {
            metadata: { name: 'proj-beta' },
            spec: {
              owner: 'u1',
              repoUrl: 'https://github.com/org/beta',
              defaultBranch: 'develop',
            },
          },
        ]),
    } as unknown as Response);

    const router = createTestRouter();
    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await router.isReady();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('proj-alpha');
    expect(wrapper.text()).toContain('proj-beta');
    expect(wrapper.text()).toContain('https://github.com/org/alpha');
    expect(wrapper.text()).toContain('https://github.com/org/beta');
  });

  it('cliquer une ligne navigue vers /p/<name>', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            metadata: { name: 'proj-alpha' },
            spec: { owner: 'u1', repoUrl: 'https://github.com/org/alpha', defaultBranch: 'main' },
          },
        ]),
    } as unknown as Response);

    const router = createTestRouter();
    await router.push('/');
    await router.isReady();

    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const row = wrapper.find('tr.row-clickable');
    await row.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/p/proj-alpha');
  });

  it('cliquer "Supprimer" appelle DELETE puis re-fetch, pas de navigation', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const u = String(url);
      const m = (init?.method ?? 'GET') as string;

      if (m === 'DELETE' && u.includes('/api/projects/to-delete')) {
        return new Response(null, { status: 204 });
      }

      if (m === 'GET' && u === '/api/projects') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([
              {
                metadata: { name: 'to-delete' },
                spec: { owner: 'u1', repoUrl: 'https://github.com/org/old', defaultBranch: null },
              },
            ]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }

      return new Response(null, { status: 500 });
    });

    const router = createTestRouter();
    await router.push('/');
    await router.isReady();

    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // DELETE a été appelé
    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/projects/to-delete'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({ method: 'DELETE' });

    // Après suppression, la liste est vide
    expect(wrapper.text()).toContain('Aucun projet');

    // Pas de navigation
    expect(router.currentRoute.value.path).toBe('/');
  });

  it('cliquer "Paramètres" navigue vers /settings', async () => {
    const router = createTestRouter();
    await router.push('/');
    await router.isReady();

    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const settingsBtn = wrapper.find('.btn-settings');
    await settingsBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/settings');
  });

  it('création en modale : dialog apparaît → remplir → créer → dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      if (url === '/api/projects' && init?.method === 'GET') {
        fetchCount++;
        // Premier GET (mount) → vide ; re-fetch après création → new-proj
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () => Promise.resolve([]),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                metadata: { name: 'new-proj' },
                spec: {
                  owner: 'u1',
                  repoUrl: 'https://github.com/org/new',
                  defaultBranch: 'main',
                },
              },
            ]),
        } as unknown as Response;
      }
      if (url === '/api/projects' && init?.method === 'POST') {
        const body = JSON.parse((init.body as string) ?? '{}');
        expect(body).toEqual({
          name: 'mon-projet',
          repoUrl: 'https://github.com/org/repo',
          defaultBranch: 'main',
        });
        return {
          ok: true,
          status: 201,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              metadata: { name: 'mon-projet' },
              spec: { owner: 'u1', repoUrl: 'https://github.com/org/repo' },
            }),
        } as unknown as Response;
      }
      return new Response(null, { status: 500 });
    });

    const router = createTestRouter();
    await router.push('/');
    await router.isReady();

    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await router.isReady();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Créer un projet" → dialog apparaît
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
    setInput(inputs[0], 'mon-projet');
    setInput(inputs[1], 'https://github.com/org/repo');
    setInput(inputs[2], 'main');

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create');
    await (dialogCreateBtn as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Re-fetch → new-proj apparaît
    expect(wrapper.text()).toContain('new-proj');

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();
  });

  it('erreur GET affiche le message d\'erreur, pas de tableau', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: false,
      status: 401,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve({ error: 'VNL-AUTH-001: Non autorisé' }),
    } as unknown as Response);

    const router = createTestRouter();
    const wrapper = mount(HomeDashboard, { global: { plugins: [router] } });
    await router.isReady();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('VNL-AUTH-001');
    expect(wrapper.text()).not.toContain('proj-alpha');
  });
});