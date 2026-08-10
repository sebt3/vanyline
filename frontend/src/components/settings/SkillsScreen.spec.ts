import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import SkillsScreen from './SkillsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('SkillsScreen', () => {
  it('affiche les 2 noms + descriptions quand GET renvoie 2 skills', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          { name: 'git-skill', description: 'Outils git' },
          { name: 'deploy-skill', description: 'Déploiement K8s' },
        ]),
    } as unknown as Response);

    const wrapper = mount(SkillsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('git-skill');
    expect(wrapper.text()).toContain('deploy-skill');
    expect(wrapper.text()).toContain('Outils git');
    expect(wrapper.text()).toContain('Déploiement K8s');
  });

  it('remplir le formulaire de création + "Créer" appelle POST avec corps attendu puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u === '/api/skills') {
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(body).toEqual({
          name: 'my-skill',
          description: 'Ma description',
          body: '# body content\nline2',
        });
        return new Response(JSON.stringify({ name: 'my-skill', description: 'Ma description', body: '# body content\nline2' }), {
          status: 201,
          headers: { 'content-type': 'application/json' },
        });
      }

      if (method === 'GET' && u === '/api/skills') {
        return new Response(
          JSON.stringify([{ name: 'my-skill', description: 'Ma description' }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(SkillsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const nameInput = wrapper.find<any>('input[aria-label="Nom du skill"]');
    const descTextarea = wrapper.find<any>('textarea[aria-label="Description"]');
    const bodyTextarea = wrapper.find<any>('textarea[aria-label="Body"]');

    await nameInput.setValue('my-skill');
    await descTextarea.setValue('Ma description');
    await bodyTextarea.setValue('# body content\nline2');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('my-skill');
  });

  it('"Modifier" appelle GET /api/skills/{name} puis charge les valeurs ; "Sauvegarder" appelle PUT', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.startsWith('/api/skills/')) {
        const name = u.replace('/api/skills/', '');
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(name).toBe('git-skill');
        expect(body).toEqual({
          description: 'updated-desc',
          body: '# updated body',
        });
        return new Response(
          JSON.stringify({
            name: 'git-skill',
            description: 'updated-desc',
            body: '# updated body',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/skills/{name}') {
        return new Response(
          JSON.stringify([{ name: 'git-skill', description: 'old desc' }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u.startsWith('/api/skills/') && u !== '/api/skills') {
        return new Response(
          JSON.stringify({
            name: 'git-skill',
            description: 'old desc',
            body: '# old body',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/skills') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{ name: 'git-skill', description: 'old desc' }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([{ name: 'git-skill', description: 'updated-desc' }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(SkillsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Modifier : git-skill');

    // Vérifier que les valeurs chargées depuis le détail sont dans le formulaire
    const editForm = wrapper.findAll<any>('.form-card');
    const editDescTextarea = editForm[1].find<any>('textarea[aria-label="Description"]');
    expect(editDescTextarea.element.value).toBe('old desc');

    const editBodyTextarea = editForm[1].find<any>('textarea[aria-label="Body"]');
    expect(editBodyTextarea.element.value).toBe('# old body');

    // Modifier les valeurs et sauvegarder
    await editDescTextarea.setValue('updated-desc');
    await editBodyTextarea.setValue('# updated body');

    const saveBtns = wrapper.findAll('.btn-success');
    await saveBtns[0].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('updated-desc');
  });

  it('cliquer "Supprimer" → DELETE /api/skills/{name} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'DELETE' && u.includes('/api/skills/to-delete')) {
        fetchCount++;
        return new Response(null, { status: 204 });
      }

      if (method === 'GET' && u === '/api/skills') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([
              { name: 'to-delete', description: 'À supprimer' },
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

    const wrapper = mount(SkillsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/skills/to-delete'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun skill');
  });
});