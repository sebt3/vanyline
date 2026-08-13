import { describe, expect, it, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import ToolsetsScreen from './ToolsetsScreen.vue';

describe('ToolsetsScreen', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockReset();
  });

  // ── Helpers ────────────────────────────────────────────────────────────────
  function jsonResponse(data: unknown): Response {
    return new Response(JSON.stringify(data), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  }

  // Helper function to create a simple route handler
  type RouteFn = (url: string, init: RequestInit | undefined) => Response | undefined;
  function mockFetch(route: RouteFn): void {
    fetchSpy.mockImplementation(async (url: string | URL, init: RequestInit | undefined) => {
      const urlStr = String(url);
      const result = route(urlStr, init);
      if (result === undefined) {
        return jsonResponse([]);
      }
      return result;
    });
  }

  // ── Test 1 : Tableau inchangé ───────────────────────────────────────────────
  it('tableau inchangé : GET renvoie 2 toolsets → noms, local_tools, serveurs MCP affichés', async () => {
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        return jsonResponse([
          {
            name: 'default',
            description: 'Outils par défaut',
            prompt: 'Prompt par défaut',
            local_tools: ['git', 'filesystem'],
            mcp: [{ server: 'code-server' }, { server: 'git-server', tools: ['diff', 'status'] }],
          },
          {
            name: 'minimal',
            local_tools: [],
            mcp: [],
          },
        ]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    expect(wrapper.text()).toContain('default');
    expect(wrapper.text()).toContain('minimal');
    expect(wrapper.text()).toContain('git');
    expect(wrapper.text()).toContain('filesystem');
    expect(wrapper.text()).toContain('code-server');
    expect(wrapper.text()).toContain('git-server');
  });

  // ── Test 2 : Create — options chargées, cocher local tools ──────────────────
  it('create — options chargées : cocher a et b → POST avec {name, local_tools, mcp: []}', async () => {
    let postBody: unknown;
    let fetchCount = 0;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse([{ name: 'default', mcp: [], local_tools: [] }]);
        }
        return jsonResponse([{ name: 'new-toolset', mcp: [], local_tools: ['a', 'b'] }]);
      }
      if (method === 'POST' && url === '/api/toolsets') {
        postBody = JSON.parse(String(init?.body));
        return jsonResponse({ name: 'new-toolset' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'a', description: 'Tool A' }, { name: 'b', description: 'Tool B' }]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([{ name: 'srv1', server_type: 'http', url: 'http://srv1', available_tools: [] }]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const nameInput = wrapper.find<HTMLInputElement>('input[aria-label="Nom du toolset"]');
    await nameInput.setValue('new-toolset');

    const allCheckboxes = wrapper.findAll<HTMLInputElement>('input[type="checkbox"]');
    expect(allCheckboxes.length).toBe(2);
    await allCheckboxes[0].setValue(true);
    await allCheckboxes[1].setValue(true);

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(postBody).toEqual({
      name: 'new-toolset',
      local_tools: ['a', 'b'],
      mcp: [],
    });
  });

  // ── Test 3 : Create — mcp row + tools ──────────────────────────────────────
  it('create — mcp : ajouter un serveur, choisir serveur, cocher 2 tools → POST avec {server, tools}', async () => {
    let fetchCount = 0;
    let postBody: unknown;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        fetchCount++;
        return jsonResponse([{ name: 'default', mcp: [], local_tools: [] }]);
      }
      if (method === 'POST' && url === '/api/toolsets') {
        postBody = JSON.parse(String(init?.body));
        return jsonResponse({ name: 'mcp-toolset' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git tool' }]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([{
          name: 'code-server', server_type: 'http', url: 'http://code',
          available_tools: ['diff', 'status', 'log'],
        }]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const nameInput = wrapper.find<HTMLInputElement>('input[aria-label="Nom du toolset"]');
    await nameInput.setValue('mcp-toolset');
    await wrapper.findAll('input[type="checkbox"]')[0].setValue(true);

    const addBtn = wrapper.find('.btn-add');
    await addBtn.trigger('click');
    await new Promise(r => setTimeout(r, 10));

    const selects = wrapper.findAll<HTMLSelectElement>('select[aria-label="Serveur MCP"]');
    expect(selects.length).toBe(1);
    await selects[0].setValue('code-server');
    await new Promise(r => setTimeout(r, 10));

    const toolCheckboxes = wrapper.findAll('input[type="checkbox"]');
    expect(toolCheckboxes.length).toBe(4);
    await toolCheckboxes[1].setValue(true);
    await toolCheckboxes[2].setValue(true);

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(postBody).toEqual({
      name: 'mcp-toolset',
      local_tools: ['git'],
      mcp: [{ server: 'code-server', tools: ['diff', 'status'] }],
    });
  });

  // ── Test 4 : État vide mcp ─────────────────────────────────────────────────
  it('état vide mcp : serveur avec available_tools: [] → message et aucune case tools', async () => {
    mockFetch((url) => {
      if (url === '/api/toolsets') {
        return jsonResponse([{ name: 'default', mcp: [], local_tools: [] }]);
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git' }]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([{
          name: 'empty-srv', server_type: 'http', url: 'http://empty', available_tools: [],
        }]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const addBtn = wrapper.find('.btn-add');
    await addBtn.trigger('click');
    await new Promise(r => setTimeout(r, 10));

    const selects = wrapper.findAll<HTMLSelectElement>('select[aria-label="Serveur MCP"]');
    await selects[0].setValue('empty-srv');
    await new Promise(r => setTimeout(r, 10));

    expect(wrapper.text()).toContain('Aucun outil disponible');
    expect(wrapper.findAll('input[type="checkbox"]').length).toBe(1);
    expect(wrapper.findAll('p.empty-state').length).toBe(1);
  });

  // ── Test 5 : Edit — chargement + sauvegarde ────────────────────────────────
  it('edit — "Modifier" pré-remplit ; "Sauvegarder" → PUT', async () => {
    let fetchCount = 0;
    let putName: string | undefined;
    let putDesc: string | undefined;
    let putPrompt: string | undefined;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse([{
            name: 'default',
            description: 'old-desc',
            prompt: 'old-prompt',
            local_tools: ['git', 'fs', 'deploy'],
            mcp: [{ server: 'code-server', tools: ['diff'] }],
          }]);
        }
        return jsonResponse([{
          name: 'default',
          description: 'updated',
          prompt: 'updated-prompt',
          local_tools: ['git', 'fs'],
          mcp: [{ server: 'code-server', tools: ['diff'] }],
        }]);
      }
      if (method === 'PUT' && url.startsWith('/api/toolsets/')) {
        const bodyObj = JSON.parse(String(init?.body)) as Record<string, unknown>;
        putName = url.replace('/api/toolsets/', '').split('?')[0];
        putDesc = bodyObj.description as string | undefined;
        putPrompt = bodyObj.prompt as string | undefined;
        return jsonResponse({ name: 'default' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([
          { name: 'git', description: 'Git' },
          { name: 'fs', description: 'Filesystem' },
          { name: 'deploy', description: 'Deploy' },
        ]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([
          { name: 'code-server', server_type: 'http', url: 'http://code', available_tools: ['diff', 'status'] },
        ]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    const editForms = wrapper.findAll<HTMLInputElement>('.form-card');
    expect(editForms.length).toBe(2);

    const allTextareas = wrapper.findAll<HTMLTextAreaElement>('textarea[aria-label="Description"]');
    expect(allTextareas.length).toBeGreaterThanOrEqual(1);
    const editDesc = allTextareas[1];
    const editPrompt = wrapper.findAll<HTMLTextAreaElement>('textarea[aria-label="Prompt"]')[1];

    await editDesc.setValue('update-desc');
    await editDesc.trigger('change');
    await new Promise(r => setTimeout(r, 10));

    await editPrompt.setValue('update-prompt');
    await editPrompt.trigger('change');
    await new Promise(r => setTimeout(r, 10));

    const saveBtn = wrapper.find('.btn-success');
    await saveBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(putName).toBe('default');
    expect(putDesc).toBe('update-desc');
    expect(putPrompt).toBe('update-prompt');
  });

  // ── Test 6 : Edit — dépendance serveur → reset tools ───────────────────────
  it('edit — changer le serveur d\'une ligne mcp → tools réinitialisés', async () => {
    let putMcpData: { server: string; tools: string[] } | undefined;
    putMcpData = { server: '', tools: [] };
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        return jsonResponse([{
          name: 'default',
          description: 'desc',
          prompt: null,
          local_tools: [],
          mcp: [{ server: 'server-a', tools: ['tool1', 'tool2'] }],
        }]);
      }
      if (method === 'PUT' && url.startsWith('/api/toolsets/')) {
        const bodyObj = JSON.parse(String(init?.body)) as Record<string, unknown>;
        if (bodyObj.mcp && Array.isArray(bodyObj.mcp) && bodyObj.mcp.length > 0) {
          const firstEntry = bodyObj.mcp[0] as { server: string; tools: string[] };
          putMcpData = { server: firstEntry.server, tools: [...firstEntry.tools] };
        }
        return jsonResponse({ name: 'default' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git' }]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([
          { name: 'server-a', server_type: 'http', url: 'http://a', available_tools: ['tool1', 'tool2'] },
          { name: 'server-b', server_type: 'http', url: 'http://b', available_tools: ['toolX', 'toolY'] },
        ]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    const selectEl = wrapper.find<HTMLSelectElement>('select[aria-label="Serveur MCP"]');
    expect((selectEl.element as HTMLSelectElement).value).toBe('server-a');

    const vm = wrapper.vm as any;
    expect(vm.editMcp).toHaveLength(1);
    expect(vm.editMcp[0].tools).toEqual(['tool1', 'tool2']);

    await selectEl.setValue('server-b');
    await new Promise(r => setTimeout(r, 10));

    expect(vm.editMcp[0].tools).toEqual([]);

    await selectEl.setValue('server-a');
    await new Promise(r => setTimeout(r, 10));
    await selectEl.setValue('server-b');
    await new Promise(r => setTimeout(r, 10));

    const saveBtn = wrapper.find('.btn-success');
    await saveBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(putMcpData?.server).toBe('server-b');
    expect(putMcpData?.tools).toEqual([]);
  });

  // ── Test 7 : Supprimer ─────────────────────────────────────────────────────
  it('Supprimer → DELETE /api/toolsets/{name} puis re-fetch', async () => {
    let fetchCount = 0;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse([{ name: 'to-delete', local_tools: ['git'], mcp: [] }]);
        }
        return jsonResponse([]);
      }
      if (method === 'DELETE' && url.startsWith('/api/toolsets/')) {
        return new Response(null, { status: 204 });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([]);
      }
      if (url === '/api/mcp-servers') {
        return jsonResponse([]);
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    const deleteBtn = wrapper.find('.btn-delete');
    expect(deleteBtn.exists()).toBe(true);
    await deleteBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Aucun toolset');
  });
});