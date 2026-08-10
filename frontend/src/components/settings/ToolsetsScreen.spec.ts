import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import ToolsetsScreen from './ToolsetsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('ToolsetsScreen', () => {
  it('affiche noms, local_tools et serveurs MCP quand GET renvoie 2 toolsets', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            name: 'default',
            description: 'Outils par d\u00e9faut',
            prompt: 'Prompt par d\u00e9faut',
            local_tools: ['git', 'filesystem'],
            mcp: [{ server: 'code-server' }, { server: 'git-server', tools: ['diff', 'status'] }],
          },
          {
            name: 'minimal',
            local_tools: [],
            mcp: [],
          },
        ]),
    } as unknown as Response);

    const wrapper = mount(ToolsetsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('default');
    expect(wrapper.text()).toContain('minimal');
    expect(wrapper.text()).toContain('git');
    expect(wrapper.text()).toContain('filesystem');
    expect(wrapper.text()).toContain('code-server');
    expect(wrapper.text()).toContain('git-server');
  });

  it('remplir le formulaire de cr\u00e9ation + "Cr\u00e9er" appelle POST avec local_tools pars\u00e9 puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let callCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u === '/api/toolsets') {
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(body).toEqual({
          name: 'new-toolset',
          description: 'une description',
          local_tools: ['a', 'b'],
        });
        return new Response(JSON.stringify({ ok: true }), {
          status: 201,
          headers: { 'content-type': 'application/json' },
        });
      }

      if (method === 'GET' && u === '/api/toolsets') {
        callCount++;
        return new Response(
          JSON.stringify([{ name: 'new-toolset', mcp: [], local_tools: [] }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(ToolsetsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const nameInput = wrapper.find<any>('input[aria-label="Nom du toolset"]');
    const descriptionTextarea = wrapper.find<any>('textarea[aria-label="Description"]');
    const promptTextarea = wrapper.find<any>('textarea[aria-label="Prompt"]');
    const localToolsInput = wrapper.find<any>('input[aria-label="Local tools"]');

    await nameInput.setValue('new-toolset');
    await descriptionTextarea.setValue('une description');
    await promptTextarea.setValue('prompt test');
    await localToolsInput.setValue('a, b');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-toolset');
  });

  it('cliquer "Modifier" charge les valeurs ; "Sauvegarder" appelle PUT avec corps update puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.startsWith('/api/toolsets/')) {
        const name = u.replace('/api/toolsets/', '');
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(name).toBe('default');
        expect(body).toEqual({
          description: 'update-desc',
          prompt: 'update-prompt',
          local_tools: ['new-tool'],
        });
        return new Response(
          JSON.stringify({
            name: 'default',
            description: 'update-desc',
            prompt: 'update-prompt',
            local_tools: ['new-tool'],
            mcp: [],
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{
              name: 'default',
              description: 'old-desc',
              prompt: 'old-prompt',
              local_tools: ['git', 'fs'],
              mcp: [{ server: 's1' }],
            }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([{
            name: 'default',
            description: 'update-desc',
            prompt: 'update-prompt',
            local_tools: ['new-tool'],
            mcp: [],
          }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(ToolsetsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();

    expect(wrapper.text()).toContain('Modifier : default');
    const editForm = wrapper.findAll<any>('.form-card');
    const editLocalToolsInput = editForm[1].find<any>('input[aria-label="Local tools"]');
    expect(editLocalToolsInput.element.value).toBe('git, fs');

    const editDescTextarea = editForm[1].find<any>('textarea[aria-label="Description"]');
    await editDescTextarea.setValue('update-desc');

    const editPromptTextarea = editForm[1].find<any>('textarea[aria-label="Prompt"]');
    await editPromptTextarea.setValue('update-prompt');

    await editLocalToolsInput.setValue('new-tool');

    const saveBtns = wrapper.findAll('.btn-success');
    await saveBtns[0].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('update-desc');
  });

  it('cliquer "Supprimer" \u2192 DELETE /{name} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/toolsets/to-delete')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{ name: 'to-delete', local_tools: ['git'], mcp: [] }]),
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

    const wrapper = mount(ToolsetsScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/toolsets/to-delete'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun toolset');
  });
});