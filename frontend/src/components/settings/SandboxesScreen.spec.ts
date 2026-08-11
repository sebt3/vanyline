import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createMemoryHistory, createRouter } from 'vue-router';
import SandboxesScreen from './SandboxesScreen.vue';

function makeRouter() {
  return createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', redirect: '/settings' },
      { path: '/settings', component: { template: '<div>Settings</div>' } },
      { path: '/ide/:sandboxName', component: { template: '<div>IDE</div>' } },
    ],
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('SandboxesScreen', () => {
  it('affiche les 2 noms, projet et branche quand GET renvoie 2 sandboxes', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            metadata: { name: 'sb-alpha' },
            spec: { project: 'proj-a', branch: 'main', suspended: false },
            status: { phase: 'Running' },
          },
          {
            metadata: { name: 'sb-beta' },
            spec: { project: 'proj-b', branch: 'develop', toolchains: [{ name: 'rust', image: 'rust:slim' }] },
            status: { phase: 'Pending' },
          },
        ]),
    } as unknown as Response);

    const router = makeRouter();
    await router.replace('/');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('sb-alpha');
    expect(wrapper.text()).toContain('sb-beta');
    expect(wrapper.text()).toContain('proj-a');
    expect(wrapper.text()).toContain('proj-b');
    expect(wrapper.text()).toContain('main');
    expect(wrapper.text()).toContain('develop');
    });

  it('remplir le formulaire + "Créer" appelle POST avec corps camelCase puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      if (url === '/api/sandboxes' && init?.method === 'GET') {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                metadata: { name: 'new-sb' },
                spec: { project: 'p', branch: 'b', suspended: false },
                status: { phase: 'Pending' },
              },
            ]),
        } as unknown as Response;
      }
      if (url === '/api/sandboxes' && init?.method === 'POST') {
        const body = JSON.parse((init.body as string) ?? '{}');
        expect(body).toEqual({
          name: 'ma-sandbox',
          project: 'mon-projet',
          branch: 'main',
        });
        return {
          ok: true,
          status: 201,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              metadata: { name: 'ma-sandbox' },
              spec: { project: 'mon-projet', branch: 'main', suspended: false },
              status: { phase: 'Pending' },
            }),
        } as unknown as Response;
      }
      return new Response(null, { status: 500 });
    });

    const router = makeRouter();
    await router.replace('/');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const nameInput = wrapper.findAll('input')[0] as any;
    const projectInput = wrapper.findAll('input')[1] as any;
    const branchInput = wrapper.findAll('input')[2] as any;

    await nameInput.setValue('ma-sandbox');
    await projectInput.setValue('mon-projet');
    await branchInput.setValue('main');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-sb');
  });

  it('cliquer "Suspendre" sur sandbox non suspendue → POST avec suspended: true, puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let callCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      callCount++;
      const m = init?.method as string;
      const u = String(url);

      if (m === 'POST' && u.includes('/api/sandboxes/sb-to-suspend/suspend')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              metadata: { name: 'sb-to-suspend' },
              spec: { project: 'p', branch: 'b', suspended: true },
              status: { phase: 'Suspended' },
            }),
        } as unknown as Response;
      }

      if (m === 'GET' && u === '/api/sandboxes') {
        if (callCount === 1) {
          // premier GET au montage → suspended: false
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  metadata: { name: 'sb-to-suspend' },
                  spec: { project: 'p', branch: 'b', suspended: false },
                  status: { phase: 'Running' },
                },
              ]),
          } as unknown as Response;
        }
        // re-fetch après POST → suspended: true
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                metadata: { name: 'sb-to-suspend' },
                spec: { project: 'p', branch: 'b', suspended: true },
                status: { phase: 'Suspended' },
              },
            ]),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const router = makeRouter();
    await router.replace('/');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Suspendre');

    const suspendBtn = wrapper.find('.btn-suspend');
    await suspendBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Reprendre');
  });

  it('cliquer "Reprendre" sur sandbox suspendue → POST avec suspended: false, puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let callCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      callCount++;
      const m = init?.method as string;
      const u = String(url);

      if (m === 'POST' && u.includes('/api/sandboxes/sb-to-resume/suspend')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              metadata: { name: 'sb-to-resume' },
              spec: { project: 'p', branch: 'b', suspended: false },
              status: { phase: 'Running' },
            }),
        } as unknown as Response;
      }

      if (m === 'GET' && u === '/api/sandboxes') {
        if (callCount === 1) {
          // premier GET au montage → suspended: true
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  metadata: { name: 'sb-to-resume' },
                  spec: { project: 'p', branch: 'b', suspended: true },
                  status: { phase: 'Suspended' },
                },
              ]),
          } as unknown as Response;
        }
        // re-fetch après POST → suspended: false
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                metadata: { name: 'sb-to-resume' },
                spec: { project: 'p', branch: 'b', suspended: false },
                status: { phase: 'Running' },
              },
            ]),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const router = makeRouter();
    await router.replace('/');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Reprendre');

    const resumeBtn = wrapper.find('.btn-suspend');
    await resumeBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Suspendre');
  });

  it('cliquer "Supprimer" → DELETE /api/sandboxes/<name> puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const m = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (m === 'DELETE' && u.includes('/api/sandboxes/sb-to-del')) {
        return new Response(null, { status: 204 });
      }
      if (m === 'GET' && u === '/api/sandboxes') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  metadata: { name: 'sb-to-del' },
                  spec: { project: 'p', branch: 'b', suspended: false },
                  status: { phase: 'Running' },
                },
              ]),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () => Promise.resolve([]),
        } as unknown as Response;
      }
      return new Response(null, { status: 500 });
    });

    const router = makeRouter();
    await router.replace('/');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
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
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucune sandbox');
  });

  it('cliquer Ouvrir navigue vers /ide/<name>', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            metadata: { name: 'sb-alpha' },
            spec: { project: 'p', branch: 'b', suspended: false },
            status: { phase: 'Running' },
          },
        ]),
    } as unknown as Response);

    const router = makeRouter();
    await router.replace('/settings');
    const wrapper = mount(SandboxesScreen, { global: { plugins: [router] } });
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const openBtn = wrapper.find('.btn-open');
    await openBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await wrapper.vm.$nextTick();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/ide/sb-alpha');
  });
});