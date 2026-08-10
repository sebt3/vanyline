import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import ProjectsScreen from './ProjectsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('ProjectsScreen', () => {
  it('affiche les 2 noms + repoUrl quand GET renvoie 2 projets', async () => {
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

    const wrapper = mount(ProjectsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('proj-alpha');
    expect(wrapper.text()).toContain('proj-beta');
    expect(wrapper.text()).toContain('https://github.com/org/alpha');
    expect(wrapper.text()).toContain('https://github.com/org/beta');
  });

  it('remplir le formulaire + "Créer" appelle POST avec corps camelCase puis re-fetch', async () => {
    let callCount = 0;
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      callCount++;
      if (url === '/api/projects' && init?.method === 'GET') {
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
      return { ok: false, status: 500 } as unknown as Response;
    });

    const wrapper = mount(ProjectsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Remplir les champs
    const nameInput = wrapper.findAll('input')[0] as any;
    const repoInput = wrapper.findAll('input')[1] as any;
    const branchInput = wrapper.findAll('input')[2] as any;

    await nameInput.setValue('mon-projet');
    await repoInput.setValue('https://github.com/org/repo');
    await branchInput.setValue('main');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Le formulaire est re-fetché → new-proj apparaît
    expect(wrapper.text()).toContain('new-proj');
  });

  it('cliquer "Supprimer" sur un projet appelée DELETE puis re-fetch', async () => {
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
        // Premier GET → retourne un projet ; re-fetch après DELETE → liste vide
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

    const wrapper = mount(ProjectsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Vérifier que DELETE a été appelé avec la bonne URL
    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/projects/to-delete'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    // Après suppression, la liste est vide
    expect(wrapper.text()).toContain('Aucun projet');
  });

  it('une erreur GET affiche le message d\'erreur', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: false,
      status: 401,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve({ error: 'VNL-AUTH-001: Non autorisé' }),
    } as unknown as Response);

    const wrapper = mount(ProjectsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('VNL-AUTH-001');
    expect(wrapper.text()).not.toContain('proj-alpha');
  });
});