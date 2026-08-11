import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import AgentsScreen from './AgentsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('AgentsScreen', () => {
  it('affiche noms, modes, modèles, skills (littéral ou noms joints) quand GET renvoie 2 agents', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
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
        ]),
    } as unknown as Response);

    const wrapper = mount(AgentsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

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

  it('remplir le formulaire de création + "Créer" appelle POST avec toolsets parsé puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u === '/api/agents') {
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(body).toEqual({
          name: 'new-agent',
          mode: 'subagent',
          model: 'claude-sonnet-4',
          toolsets: ['t1', 't2'],
          skills: 'auto',
        });
        return new Response(JSON.stringify({ ok: true }), {
          status: 201,
          headers: { 'content-type': 'application/json' },
        });
      }

      if (method === 'GET' && u === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{ name: 'existing', mode: 'primary', model: 'gpt-4', toolsets: [], skills: 'auto', system_prompt: '' }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([{ name: 'new-agent', mode: 'subagent', model: 'claude-sonnet-4', toolsets: ['t1', 't2'], skills: 'auto', system_prompt: '' }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(AgentsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const nameInput = wrapper.find<any>('input[aria-label="Nom de l\'agent"]');
    const modeSelect = wrapper.find<any>('select[aria-label="Mode"]');
    const modelInput = wrapper.find<any>('input[aria-label="Modèle"]');
    const toolsetsInput = wrapper.find<any>('input[aria-label="Toolsets"]');
    const skillsSelect = wrapper.find<any>('.card select[aria-label="Skills"]');

    await nameInput.setValue('new-agent');
    await modeSelect.setValue('subagent');
    await modelInput.setValue('claude-sonnet-4');
    await toolsetsInput.setValue('t1, t2');
    await skillsSelect.setValue('auto');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-agent');
  });

  it('cliquer "Modifier" charge les valeurs ; "Sauvegarder" appelle PUT avec corps update puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    let capturedPutBody: unknown = null;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.startsWith('/api/agents/')) {
        capturedPutBody = JSON.parse(String((init as RequestInit)?.body ?? '{}'));
        return new Response(
          JSON.stringify({
            name: 'edit-me',
            description: 'updated desc',
            mode: 'all',
            model: 'new-model',
            toolsets: ['new-t'],
            skills: 'none',
            system_prompt: 'updated prompt',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{
              name: 'edit-me',
              description: 'old desc',
              mode: 'primary',
              model: 'old-model',
              toolsets: ['git'],
              skills: 'auto',
              system_prompt: 'old prompt',
            }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([{
            name: 'edit-me',
            description: 'updated desc',
            mode: 'all',
            model: 'new-model',
            toolsets: ['new-t'],
            skills: 'none',
            system_prompt: 'updated prompt',
          }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(AgentsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();

    expect(wrapper.text()).toContain('Modifier : edit-me');
    const editForm = wrapper.findAll<any>('.form-card');

    const editModelInput = editForm[1].find<HTMLInputElement>(
      'input[aria-label="Modèle"]',
    );
    expect(editModelInput!.element.value).toBe('old-model');

    const editToolsetsInput = editForm[1].find<HTMLInputElement>(
      'input[aria-label="Toolsets"]',
    );
    expect(editToolsetsInput!.element.value).toBe('git');

    const editDescriptionTextarea = editForm[1].find<HTMLTextAreaElement>(
      'textarea[aria-label="Description"]',
    );

    await editDescriptionTextarea!.setValue('updated desc');
    await editModelInput!.setValue('new-model');
    await editToolsetsInput!.setValue('new-t');

    const editModeSelect = editForm[1].find<any>('select[aria-label="Mode"]');
    await editModeSelect.setValue('all');

    const editSkillsSelect = editForm[1].find<any>('select[aria-label="Skills"]');
    await editSkillsSelect.setValue('none');

    const editSystemTextarea = editForm[1].find<HTMLTextAreaElement>(
      'textarea[aria-label="System prompt"]',
    );
    await editSystemTextarea!.setValue('updated prompt');

    const saveBtns = wrapper.findAll('.btn-success');
    await saveBtns[0].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('updated desc');
    expect(capturedPutBody).toEqual({
      description: 'updated desc',
      mode: 'all',
      model: 'new-model',
      toolsets: ['new-t'],
      skills: 'none',
      system_prompt: 'updated prompt',
    });
  });

  it('cliquer "Supprimer" → DELETE /{name} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/agents/to-delete')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/agents') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{ name: 'to-delete', mode: 'primary', model: 'gpt-4', toolsets: [], skills: 'auto', system_prompt: '' }]),
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

    const wrapper = mount(AgentsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/agents/to-delete'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun agent');
  });
});