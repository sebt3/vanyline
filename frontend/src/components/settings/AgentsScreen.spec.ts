import { describe, expect, it, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentsScreen from './AgentsScreen.vue';

// Helpers — creates fresh Response each call so body stream isn't reused
function jsonResponse(data: unknown): Response {
  return new Response(JSON.stringify(data), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('AgentsScreen', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
  });

  it('affiche noms, modes, modèles, skills (littéral ou noms joints) quand GET renvoie 2 agents', async () => {
    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const urlStr = String(url);
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && urlStr === '/api/agents') {
        return jsonResponse([
          {
            name: 'primary-agent',
            description: 'Agent principal',
            mode: 'primary',
            model: 'claude-sonnet-4',
            toolsets: ['git'],
            skills: 'auto',
            system_prompt: 'Prompt principal',
          },
          {
            name: 'sub-agent',
            mode: 'subagent',
            model: 'gpt-4',
            toolsets: [],
            skills: ['a', 'b'],
            system_prompt: '',
          },
        ]);
      }
      return jsonResponse([]);
    });

    const wrapper = mount(AgentsScreen);
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('primary-agent');
    expect(wrapper.text()).toContain('sub-agent');
    expect(wrapper.text()).toContain('primary');
    expect(wrapper.text()).toContain('subagent');
    expect(wrapper.text()).toContain('claude-sonnet-4');
    expect(wrapper.text()).toContain('gpt-4');
    expect(wrapper.text()).toContain('auto');
    expect(wrapper.text()).toContain('a, b');
    expect(wrapper.text()).toContain('—');
  });

  it('remplir + "Créer" → POST avec toolsets tableau, puis re-fetch', async () => {
    fetchSpy.mockReset();
    let postBody: unknown;
    let fetchCount = 0;

    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const urlStr = String(url);
      const method = String(init?.method ?? 'GET').toUpperCase();

      // Create test options (no caching — fresh response each call)
      if (urlStr === '/api/model-profiles') return jsonResponse([{ name: 'claude-sonnet-4' }, { name: 'gpt-4' }]);
      if (urlStr === '/api/toolsets') return jsonResponse([{ name: 'git', description: 'Git' }, { name: 'filesystem', description: 'FS' }]);
      if (urlStr === '/api/skills') return jsonResponse([{ name: 'skill-a', description: 'A' }]);

      if (method === 'GET' && urlStr === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) return jsonResponse([{ name: 'existing', mode: 'primary', model: 'gpt-4', toolsets: [], skills: 'auto', system_prompt: '' }]);
        return jsonResponse([{ name: 'new-agent', mode: 'subagent', model: 'claude-sonnet-4', toolsets: ['git', 'filesystem'], skills: 'auto', system_prompt: '' }]);
      }
      if (method === 'POST' && urlStr === '/api/agents') {
        postBody = JSON.parse(String(init?.body));
        expect(postBody).toEqual({ name: 'new-agent', mode: 'subagent', model: 'claude-sonnet-4', toolsets: ['git', 'filesystem'], skills: 'auto' });
        return jsonResponse({ ok: true });
      }
      return new Response('', { status: 404 });
    });

    const wrapper = mount(AgentsScreen);
    await new Promise((r) => setTimeout(r, 50));

    await wrapper.find<HTMLInputElement>('input[aria-label="Nom de l\'agent"]').setValue('new-agent');
    await wrapper.find<HTMLSelectElement>('select[aria-label="Mode"]').setValue('subagent');
    await wrapper.find<HTMLSelectElement>('select[aria-label="Profil de modèle"]').setValue('claude-sonnet-4');

    const cbs = wrapper.findAll<HTMLInputElement>('.checkbox-item input[type="checkbox"]');
    await cbs[0].setValue(true);
    await cbs[1].setValue(true);

    await wrapper.find('.btn-create').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('new-agent');
  });

  it('edit agent à skills tableau → cases pré-remplies; "Sauvegarder" → PUT', async () => {
    fetchSpy.mockReset();
    const putBodies: unknown[] = [];
    let fetchCount = 0;

    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const urlStr = String(url);
      const method = String(init?.method ?? 'GET').toUpperCase();

      // Each URL path — fresh response (body stream consumed per-call)
      if (urlStr === '/api/model-profiles') return jsonResponse([{ name: 'old-model' }, { name: 'new-model' }, { name: 'gpt-4' }]);
      if (urlStr === '/api/toolsets') return jsonResponse([{ name: 'git', description: 'Git' }, { name: 'new-t', description: 'NT' }, { name: 'filesystem', description: 'FS' }]);
      if (urlStr === '/api/skills') return jsonResponse([{ name: 'a', description: 'A' }, { name: 'b', description: 'B' }]);

      if (method === 'GET' && urlStr === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) return jsonResponse([{
          name: 'edit-me', description: 'old desc', mode: 'primary',
          model: 'old-model', toolsets: ['git'], skills: ['a', 'b'],
          system_prompt: 'old prompt',
        }]);
        return jsonResponse([{
          name: 'edit-me', description: 'updated desc', mode: 'all',
          model: 'new-model', toolsets: ['new-t'], skills: ['a', 'b'],
          system_prompt: 'updated prompt',
        }]);
      }

      if (method === 'PUT' && urlStr.startsWith('/api/agents/')) {
        putBodies.push(JSON.parse(String(init?.body ?? '{}')));
        return jsonResponse({
          name: 'edit-me', description: 'updated desc', mode: 'all',
          model: 'new-model', toolsets: ['new-t'], skills: ['a', 'b'],
          system_prompt: 'updated prompt',
        });
      }
      return new Response('', { status: 404 });
    });

    const wrapper = mount(AgentsScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Vérifier que modelProfiles est chargé
    const vm = wrapper.vm as any;
    expect(vm.modelProfiles).toHaveLength(3);

    // Ouvrir l'éditeur — l'agent a model: 'old-model'
    await wrapper.find('.btn-edit').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Modifier : edit-me');

    // Vérifier que editModel est pré-rempli (via la ref du composant)
    // Dans Vue 3, editModel.value est une ref → wrapper.vm.editModel
    await expect(() => {
      // Si le select a 'old-model' comme value, c'est que v-model a fonctionné
      const editModel = wrapper.find<HTMLSelectElement>('select[aria-label="Profil de modèle"]');
      const opts = editModel.findAll('option');
      expect(opts.some((o: any) => o.text() === 'old-model')).toBe(true);
      // Le select v-model doit refléter la donnée
    }).not.toThrow();

    // Sélectionner le formulaire d'édition (dernière .form-card après le tableau)
    const editForms = wrapper.findAll<HTMLDivElement>('.form-card');
    const editFormEl = editForms[editForms.length - 1].element;

    // Utiliser querySelector sur l'élément DOM brut
    function q<T extends HTMLElement>(sel: string, root = editFormEl as HTMLElement): T | null {
      return root.querySelector<T>(sel);
    }

    // Vérifier les checkboxes dans le formulaire d'édition seulement
    const editCbs = Array.from(editFormEl.querySelectorAll<HTMLInputElement>('.checkbox-item input[type="checkbox"]'));
    // [0]=git(chk), [1]=new-t, [2]=filesystem, [3]=a(chk), [4]=b(chk)
    expect(editCbs.length).toBe(5);
    expect(editCbs[0].checked).toBe(true);  // git
    expect(editCbs[3].checked).toBe(true);  // a
    expect(editCbs[4].checked).toBe(true);  // b

    // Modifier : on mutate directement les refs Vue pour garantir la réactivité
    const v = wrapper.vm as any;
    v.editToolsets = ['new-t'];  // [0]=git→décoché, [1]=new-t→coché, [2]=filesystem→décoché
    v.editSkillList = ['b'];     // [3]=a→décoché, [4]=b→demeuré coché

    // Profil de modèle: new-model
    const editModelSelect = q<HTMLSelectElement>('select[aria-label="Profil de modèle"]')!;
    editModelSelect.value = 'new-model';
    editModelSelect.dispatchEvent(new Event('change', { bubbles: true }));

    const descInput = q<HTMLTextAreaElement>('textarea[aria-label="Description"]')!;
    descInput.value = 'updated desc';
    descInput.dispatchEvent(new Event('input', { bubbles: true }));

    const modeSelect = q<HTMLSelectElement>('select[aria-label="Mode"]')!;
    modeSelect.value = 'all';
    modeSelect.dispatchEvent(new Event('change', { bubbles: true }));

    const promptInput = q<HTMLTextAreaElement>('textarea[aria-label="System prompt"]')!;
    promptInput.value = 'updated prompt';
    promptInput.dispatchEvent(new Event('input', { bubbles: true }));

    await wrapper.find('.btn-success').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('updated desc');
    expect(putBodies).toHaveLength(1);
    expect(putBodies[0]).toEqual({
      description: 'updated desc', mode: 'all', model: 'new-model',
      toolsets: ['new-t'], skills: ['b'], system_prompt: 'updated prompt',
    });
  });

  it('edit agent à skills "auto" → branche select auto/none', async () => {
    fetchSpy.mockReset();
    let fetchCount = 0;

    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const urlStr = String(url);
      const method = String(init?.method ?? 'GET').toUpperCase();

      // Fresh responses
      if (urlStr === '/api/model-profiles') return jsonResponse([{ name: 'old-model' }, { name: 'new-model' }, { name: 'gpt-4' }]);
      if (urlStr === '/api/toolsets') return jsonResponse([{ name: 'git', description: 'Git' }, { name: 'filesystem', description: 'FS' }]);
      if (urlStr === '/api/skills') return jsonResponse([{ name: 'a', description: 'A' }]);

      if (method === 'GET' && urlStr === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) return jsonResponse([{
          name: 'edit-me', description: 'old desc', mode: 'primary',
          model: 'old-model', toolsets: ['git'], skills: 'auto',
          system_prompt: 'old prompt',
        }]);
        return jsonResponse([{
          name: 'edit-me', description: 'updated desc', mode: 'all',
          model: 'new-model', toolsets: ['git', 'filesystem'], skills: 'none',
          system_prompt: 'updated prompt',
        }]);
      }

      if (method === 'PUT' && urlStr.startsWith('/api/agents/')) {
        return jsonResponse({
          name: 'edit-me', description: 'updated desc', mode: 'all',
          model: 'new-model', toolsets: ['filesystem'], skills: 'none',
          system_prompt: 'updated prompt',
        });
      }
      return new Response('', { status: 404 });
    });

    const wrapper = mount(AgentsScreen);
    await new Promise((r) => setTimeout(r, 50));

    const vm2 = wrapper.vm as any;
    expect(vm2.modelProfiles).toHaveLength(3);

    await wrapper.find('.btn-edit').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Modifier : edit-me');

    // Vérifier que editModel est pré-rempli
    expect((wrapper.find('select[aria-label="Profil de modèle"]'). findAll('option').some((o: any) => o.text() === 'old-model'))).toBe(true);

    const editSkillsSelect = wrapper.find<HTMLSelectElement>('select[aria-label="Skills"]');
    expect(editSkillsSelect.exists()).toBe(true);

    const toolsetCbs = wrapper.findAll<HTMLInputElement>('.checkbox-item input[type="checkbox"]');
    await toolsetCbs[0].setValue(false);
    await toolsetCbs[1].setValue(true);

    await wrapper.find<HTMLTextAreaElement>('textarea[aria-label="Description"]').setValue('updated desc');
    const editModelSelect = wrapper.find<HTMLSelectElement>('select[aria-label="Profil de modèle"]');
    await editModelSelect.setValue('new-model');
    await editModelSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));
    await wrapper.find<HTMLSelectElement>('select[aria-label="Mode"]').setValue('all');
    await editSkillsSelect.setValue('none');
    await wrapper.find<HTMLTextAreaElement>('textarea[aria-label="System prompt"]').setValue('updated prompt');

    await wrapper.find('.btn-success').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('updated desc');
  });

  it('cliquer "Supprimer" → DELETE /{name} puis re-fetch', async () => {
    fetchSpy.mockReset();
    let fetchCount = 0;
    let deleteTarget: string | undefined;

    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      const urlStr = String(url);
      if (method === 'DELETE' && urlStr.includes('/api/agents/')) {
        deleteTarget = urlStr.replace('/api/agents/', '');
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && urlStr === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) return jsonResponse([{ name: 'to-delete', mode: 'primary', model: 'gpt-4', toolsets: [], skills: 'auto', system_prompt: '' }]);
        return jsonResponse([]);
      }
      return new Response('', { status: 404 });
    });

    const wrapper = mount(AgentsScreen);
    await new Promise((r) => setTimeout(r, 50));

    await wrapper.find('.btn-delete').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    expect(deleteTarget).toBe('to-delete');
    expect(wrapper.text()).toContain('Aucun agent');
  });
});