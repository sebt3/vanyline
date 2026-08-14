import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentsScreen from './AgentsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
});

// Helpers — creates fresh Response each call so body stream isn't reused
function jsonResponse(data: unknown): Response {
  return new Response(JSON.stringify(data), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('AgentsScreen', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  it('affiche noms, modes, modèles, skills (littéral ou noms joints) quand GET renvoie 2 agents', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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

    // Ouvrir la modale de création
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise((r) => setTimeout(r, 10));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    // Remplir les champs du dialog
    const nameInput = dialog!.querySelector<HTMLInputElement>('input[aria-label="Nom de l\'agent"]')!;
    nameInput.value = 'new-agent';
    nameInput.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modeSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Mode"]')!;
    modeSelect.value = 'subagent';
    modeSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Profil de modèle"]')!;
    modelSelect.value = 'claude-sonnet-4';
    modelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const checkboxes = dialog!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    expect(checkboxes.length).toBe(2);
    checkboxes[0].checked = true;
    checkboxes[0].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));
    checkboxes[1].checked = true;
    checkboxes[1].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog!.querySelector<HTMLButtonElement>('.btn-create');
    await dialogCreateBtn!.click();
    await new Promise((r) => setTimeout(r, 50));

    // Dialog fermé — vérifier via l'état du composant
    expect((wrapper.vm as any).createModalOpen).toBe(false);

    // Re-fetch → nouvelle donnée affichée
    expect(wrapper.text()).toContain('new-agent');
  });

  it('edit agent à skills tableau → cases pré-remplies; "Sauvegarder" → PUT', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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

    // Ouvrir l'éditeur
    await wrapper.find('.btn-edit').trigger('click');
    await new Promise((r) => setTimeout(r, 50));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog!.textContent).toContain('Modifier : edit-me');

    // Vérifier 5 checkboxes dans le dialog (git + new-t + filesystem + a + b)
    const editCbs = dialog!.querySelectorAll<HTMLInputElement>('.checkbox-item input[type="checkbox"]');
    expect(editCbs.length).toBe(5);
    expect(editCbs[0].checked).toBe(true);  // git
    expect(editCbs[3].checked).toBe(true);  // a
    expect(editCbs[4].checked).toBe(true);  // b

    // Modifier : on mutate directement les refs Vue pour garantir la réactivité
    const v = wrapper.vm as any;
    v.editToolsets = ['new-t'];  // [0]=git→décoché, [1]=new-t→coché, [2]=filesystem→décoiché
    v.editSkillList = ['b'];     // [3]=a→décoché, [4]=b→demeuré coché

    // Profil de modèle: new-model
    const editModelSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Profil de modèle"]')!;
    editModelSelect.value = 'new-model';
    editModelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const descInput = dialog!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Description"]')!;
    descInput.value = 'updated desc';
    descInput.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modeSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Mode"]')!;
    modeSelect.value = 'all';
    modeSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const promptInput = dialog!.querySelector<HTMLTextAreaElement>('textarea[aria-label="System prompt"]')!;
    promptInput.value = 'updated prompt';
    promptInput.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Cliquer "Sauvegarder" du dialog
    const saveBtn = dialog!.querySelector<HTMLButtonElement>('.btn-success')!;
    await saveBtn.click();
    await new Promise((r) => setTimeout(r, 50));

    // Dialog fermé — via état du composant
    expect((wrapper.vm as any).editModalOpen).toBe(false);

    expect(wrapper.text()).toContain('updated desc');
    expect(putBodies).toHaveLength(1);
    expect(putBodies[0]).toEqual({
      description: 'updated desc', mode: 'all', model: 'new-model',
      toolsets: ['new-t'], skills: ['b'], system_prompt: 'updated prompt',
    });
  });

  it('edit agent à skills "auto" → branche select auto/none', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog!.textContent).toContain('Modifier : edit-me');

    // Vérifier que le select Skills existe (branche select, pas checkbox-list)
    const editSkillsSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Skills"]');
    expect(editSkillsSelect).toBeTruthy();

    const toolsetCbs = dialog!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    toolsetCbs[0].checked = false;
    toolsetCbs[0].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));
    toolsetCbs[1].checked = true;
    toolsetCbs[1].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Modifier les champs du dialog
    const descInput = dialog!.querySelector<HTMLTextAreaElement>('textarea[aria-label="Description"]')!;
    descInput.value = 'updated desc';
    descInput.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const editModelSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Profil de modèle"]')!;
    editModelSelect.value = 'new-model';
    editModelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modeSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Mode"]')!;
    modeSelect.value = 'all';
    modeSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    editSkillsSelect!.value = 'none';
    editSkillsSelect!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const promptInput = dialog!.querySelector<HTMLTextAreaElement>('textarea[aria-label="System prompt"]')!;
    promptInput.value = 'updated prompt';
    promptInput.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Cliquer "Sauvegarder" du dialog
    const saveBtn = dialog!.querySelector<HTMLButtonElement>('.btn-success')!;
    await saveBtn.click();
    await new Promise((r) => setTimeout(r, 50));

    expect((wrapper.vm as any).editModalOpen).toBe(false);
    expect(wrapper.text()).toContain('updated desc');
  });

  it('cliquer "Supprimer" → DELETE /{name} puis re-fetch', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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
