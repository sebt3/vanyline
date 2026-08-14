import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import SkillsScreen from './SkillsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
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

  it('création en modale : dialog apparaît → remplir → créer → dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u === '/api/skills') {
        const body = JSON.parse((init?.body as string | undefined) ?? '{}');
        expect(body).toEqual({
          name: 'my-skill',
          description: 'Ma description',
          body: '# body content\nline2',
        });
        return new Response(
          JSON.stringify({ name: 'my-skill', description: 'Ma description', body: '# body content\nline2' }),
          { status: 201, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/skills') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{ name: 'existing-skill', description: 'Existant' }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
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

    // Cliquer "Créer un skill" → dialog apparaît
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="dialog"]')).toBeTruthy();

    const dialog = document.querySelector('[role="dialog"]')!;

    // Remplir les champs du dialog
    const nameInput = dialog.querySelector('input[aria-label="Nom du skill"]')!;
    (nameInput as HTMLInputElement).value = 'my-skill';
    nameInput.dispatchEvent(new Event('input', { bubbles: true }));

    const descTextarea = dialog.querySelector('textarea[aria-label="Description"]')!;
    (descTextarea as HTMLTextAreaElement).value = 'Ma description';
    descTextarea.dispatchEvent(new Event('input', { bubbles: true }));

    const bodyTextarea = dialog.querySelector('textarea[aria-label="Body"]')!;
    (bodyTextarea as HTMLTextAreaElement).value = '# body content\nline2';
    bodyTextarea.dispatchEvent(new Event('input', { bubbles: true }));

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create')!;
    await (dialogCreateBtn as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Dialog fermé — l'état du composant indique la fermeture
    expect((wrapper.vm as any).createModalOpen).toBe(false);

    // Nouvelle donnée apparaît
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
        const body = JSON.parse((init?.body as string | undefined) ?? '{}');
        expect(name).toBe('git-skill');
        expect(body).toEqual({
          description: 'updated-desc',
          body: '# updated body',
        });
        return new Response(
          JSON.stringify({ name: 'git-skill', description: 'updated-desc', body: '# updated body' }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u.startsWith('/api/skills/') && u !== '/api/skills') {
        return new Response(
          JSON.stringify({ name: 'git-skill', description: 'old desc', body: '# old body' }),
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

    // Le GET détail est async → attendre sa résolution et le cycle de rendu Vue
    await flushPromises();
    await wrapper.vm.$nextTick();
    await wrapper.vm.$nextTick();

    // Vérifier l'état du composant (la modale est ouverte)
    expect((wrapper.vm as any).editModalOpen).toBe(true);

    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog).toBeTruthy();
    expect(dialog.textContent).toContain('Modifier : git-skill');

    // Vérifier que les valeurs chargées depuis le détail sont dans le formulaire
    const editDescTextarea = dialog.querySelector('textarea[aria-label="Description"]')!;
    expect((editDescTextarea as HTMLTextAreaElement).value).toBe('old desc');

    const editBodyTextarea = dialog.querySelector('textarea[aria-label="Body"]')!;
    expect((editBodyTextarea as HTMLTextAreaElement).value).toBe('# old body');

    // Modifier les valeurs et sauvegarder
    (editDescTextarea as HTMLTextAreaElement).value = 'updated-desc';
    editDescTextarea.dispatchEvent(new Event('input', { bubbles: true }));
    (editBodyTextarea as HTMLTextAreaElement).value = '# updated body';
    editBodyTextarea.dispatchEvent(new Event('input', { bubbles: true }));

    const saveBtn = dialog.querySelector('.btn-success')!;
    await (saveBtn as HTMLElement).click();
    await flushPromises();
    await wrapper.vm.$nextTick();

    // Dialog fermé
    expect((wrapper.vm as any).editModalOpen).toBe(false);

    // Re-fetch → valeurs mises à jour
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
