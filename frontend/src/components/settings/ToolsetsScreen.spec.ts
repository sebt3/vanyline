import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import ToolsetsScreen from './ToolsetsScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
});

describe('ToolsetsScreen', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

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
    fetchSpy = vi.spyOn(globalThis, 'fetch');
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
  it('tableau inchangé : GET renvoie PagedResult → noms, local_tools, serveurs MCP affichés', async () => {
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        return jsonResponse({
          items: [
            {
              id: 1,
              name: 'default',
              description: 'Outils par défaut',
              prompt: 'Prompt par défaut',
              local_tools: ['git', 'filesystem'],
              mcp: [{ server: 'code-server' }, { server: 'git-server', tools: ['diff', 'status'] }],
            },
            {
              id: 2,
              name: 'minimal',
              local_tools: [],
              mcp: [],
            },
          ],
          page: 1,
          per_page: 100,
          total_items: 2,
          total_pages: 1,
        });
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
      if (method === 'GET' && url === '/api/v1/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse({
            items: [{ id: 1, name: 'default', mcp: [], local_tools: [] }],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          });
        }
        return jsonResponse({
          items: [{ id: 2, name: 'new-toolset', mcp: [], local_tools: ['a', 'b'] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (method === 'POST' && url === '/api/v1/toolsets') {
        postBody = JSON.parse(String(init?.body));
        return jsonResponse({ id: 2, name: 'new-toolset' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'a', description: 'Tool A' }, { name: 'b', description: 'Tool B' }]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({
          items: [{ id: 1, name: 'srv1', server_type: 'http', url: 'http://srv1', available_tools: [] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Ouvrir la modale de création
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise(r => setTimeout(r, 10));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    // Remplir les champs du dialog
    const nameInput = dialog!.querySelector<HTMLInputElement>('input[aria-label="Nom du toolset"]');
    nameInput!.value = 'new-toolset';
    nameInput!.dispatchEvent(new Event('input', { bubbles: true }));
    nameInput!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    const checkboxes = dialog!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    expect(checkboxes.length).toBe(2);
    checkboxes[0].checked = true;
    checkboxes[0].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));
    checkboxes[1].checked = true;
    checkboxes[1].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Cliquer Créer du dialog
    const dialogCreateBtn = dialog!.querySelector<HTMLButtonElement>('.btn-create');
    await dialogCreateBtn!.click();
    await new Promise(r => setTimeout(r, 50));

    expect(postBody).toEqual({
      name: 'new-toolset',
      local_tools: ['a', 'b'],
      mcp: [],
    });

    // Dialog fermé
    expect((wrapper.vm as any).createModalOpen).toBe(false);
  });

  // ── Test 3 : Create — mcp row + tools ──────────────────────────────────────
  it('create — mcp : ajouter un serveur, choisir serveur, cocher 2 tools → POST avec {server, tools}', async () => {
    let fetchCount = 0;
    let postBody: unknown;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        fetchCount++;
        return jsonResponse({
          items: [{ id: 1, name: 'default', mcp: [], local_tools: [] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (method === 'POST' && url === '/api/v1/toolsets') {
        postBody = JSON.parse(String(init?.body));
        return jsonResponse({ id: 2, name: 'mcp-toolset' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git tool' }]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({
          items: [{ id: 1, name: 'code-server', server_type: 'http', url: 'http://code', available_tools: ['diff', 'status', 'log'] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Ouvrir la modale
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise(r => setTimeout(r, 10));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    // Remplir le nom et cocher 1 local tool
    const nameInput = dialog!.querySelector<HTMLInputElement>('input[aria-label="Nom du toolset"]');
    nameInput!.value = 'mcp-toolset';
    nameInput!.dispatchEvent(new Event('input', { bubbles: true }));
    nameInput!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));
    const firstCheckbox = dialog!.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    firstCheckbox.checked = true;
    firstCheckbox.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Ajouter un serveur MCP
    const addBtn = dialog!.querySelector<HTMLButtonElement>('button.btn-add');
    await addBtn!.click();
    await new Promise(r => setTimeout(r, 10));

    // Choisir le serveur
    const select = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Serveur MCP"]');
    select!.value = 'code-server';
    select!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Cocher 2 MCP tools (checkboxes[1] = 'diff', checkboxes[2] = 'status')
    const allCheckboxes = dialog!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    expect(allCheckboxes.length).toBe(4);
    allCheckboxes[1].checked = true;
    allCheckboxes[1].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 20));
    allCheckboxes[2].checked = true;
    allCheckboxes[2].dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 20));

    // Cliquer Créer du dialog
    const dialogCreateBtn = dialog!.querySelector<HTMLButtonElement>('.btn-create');
    await dialogCreateBtn!.click();
    await new Promise(r => setTimeout(r, 50));

    expect(postBody).toEqual({
      name: 'mcp-toolset',
      local_tools: ['git'],
      mcp: [{ server: 'code-server', tools: ['diff', 'status'] }],
    });

    expect((wrapper.vm as any).createModalOpen).toBe(false);
  });

  // ── Test 4 : État vide mcp ─────────────────────────────────────────────────
  it('état vide mcp : serveur avec available_tools: [] → message et aucune case tools', async () => {
    mockFetch((url) => {
      if (url === '/api/v1/toolsets') {
        return jsonResponse({
          items: [{ id: 1, name: 'default', mcp: [], local_tools: [] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git' }]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({
          items: [{ id: 1, name: 'empty-srv', server_type: 'http', url: 'http://empty', available_tools: [] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Ouvrir la modale
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await new Promise(r => setTimeout(r, 10));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    // Ajouter un serveur MCP via le dialog
    const addBtn = dialog!.querySelector<HTMLButtonElement>('button.btn-add');
    await addBtn!.click();
    await new Promise(r => setTimeout(r, 10));

    // Choisir le serveur
    const select = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Serveur MCP"]');
    select!.value = 'empty-srv';
    select!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Vérifier dans le dialog téléporté
    expect(dialog!.textContent).toContain('Aucun outil disponible');
    expect(dialog!.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').length).toBe(1);
    expect(dialog!.querySelectorAll('p.empty-state').length).toBe(1);
  });

  // ── Test 5 : Edit — chargement + sauvegarde ────────────────────────────────
  it('edit — "Modifier" pré-remplit ; "Sauvegarder" → PUT', async () => {
    let fetchCount = 0;
    let putId: number | undefined;
    let putDesc: string | undefined;
    let putPrompt: string | undefined;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse({
            items: [{
              id: 1,
              name: 'default',
              description: 'old-desc',
              prompt: 'old-prompt',
              local_tools: ['git', 'fs', 'deploy'],
              mcp: [{ server: 'code-server', tools: ['diff'] }],
            }],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          });
        }
        return jsonResponse({
          items: [{
            id: 1,
            name: 'default',
            description: 'updated',
            prompt: 'updated-prompt',
            local_tools: ['git', 'fs'],
            mcp: [{ server: 'code-server', tools: ['diff'] }],
          }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (method === 'PUT' && url.startsWith('/api/v1/toolsets/')) {
        const bodyObj = JSON.parse(String(init?.body)) as Record<string, unknown>;
        putId = parseInt(url.replace('/api/v1/toolsets/', ''), 10);
        putDesc = bodyObj.description as string | undefined;
        putPrompt = bodyObj.prompt as string | undefined;
        return jsonResponse({ id: 1, name: 'default' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([
          { name: 'git', description: 'Git' },
          { name: 'fs', description: 'Filesystem' },
          { name: 'deploy', description: 'Deploy' },
        ]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({
          items: [{ id: 1, name: 'code-server', server_type: 'http', url: 'http://code', available_tools: ['diff', 'status'] }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Cliquer "Modifier" sur une ligne
    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    // Trouver les textareas dans le dialog
    const allTextareas = dialog!.querySelectorAll<HTMLTextAreaElement>('textarea[aria-label="Description"]');
    expect(allTextareas.length).toBe(1);
    const editDesc = allTextareas[0];
    const allPromptTextareas = dialog!.querySelectorAll<HTMLTextAreaElement>('textarea[aria-label="Prompt"]');
    expect(allPromptTextareas.length).toBe(1);
    const editPrompt = allPromptTextareas[0];

    // Vérifier les valeurs pré-remplies
    expect((editDesc as HTMLTextAreaElement).value).toBe('old-desc');
    expect((editPrompt as HTMLTextAreaElement).value).toBe('old-prompt');

    // Modifier les valeurs
    (editDesc as HTMLTextAreaElement).value = 'update-desc';
    editDesc!.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    (editPrompt as HTMLTextAreaElement).value = 'update-prompt';
    editPrompt!.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Cliquer Sauvegarder du dialog
    const saveBtn = dialog!.querySelector<HTMLButtonElement>('.btn-success')!;
    await saveBtn.click();
    await new Promise(r => setTimeout(r, 50));

    expect(putId).toBe(1);
    expect(putDesc).toBe('update-desc');
    expect(putPrompt).toBe('update-prompt');

    expect((wrapper.vm as any).editModalOpen).toBe(false);
  });

  // ── Test 6 : Edit — dépendance serveur → reset tools ───────────────────────
  it('edit — changer le serveur d\'une ligne mcp → tools réinitialisés', async () => {
    let putMcpData: { server: string; tools: string[] } | undefined;
    putMcpData = { server: '', tools: [] };
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        return jsonResponse({
          items: [{
            id: 1,
            name: 'default',
            description: 'desc',
            prompt: null,
            local_tools: [],
            mcp: [{ server: 'server-a', tools: ['tool1', 'tool2'] }],
          }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (method === 'PUT' && url.startsWith('/api/v1/toolsets/')) {
        const bodyObj = JSON.parse(String(init?.body)) as Record<string, unknown>;
        if (bodyObj.mcp && Array.isArray(bodyObj.mcp) && bodyObj.mcp.length > 0) {
          const firstEntry = bodyObj.mcp[0] as { server: string; tools: string[] };
          putMcpData = { server: firstEntry.server, tools: [...firstEntry.tools] };
        }
        return jsonResponse({ id: 1, name: 'default' });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([{ name: 'git', description: 'Git' }]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({
          items: [
            { id: 1, name: 'server-a', server_type: 'http', url: 'http://a', available_tools: ['tool1', 'tool2'] },
            { id: 2, name: 'server-b', server_type: 'http', url: 'http://b', available_tools: ['toolX', 'toolY'] },
          ],
          page: 1,
          per_page: 100,
          total_items: 2,
          total_pages: 1,
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Cliquer "Modifier"
    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await new Promise(r => setTimeout(r, 50));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    const selectEl = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Serveur MCP"]')!;
    expect(selectEl.value).toBe('server-a');

    const vm = wrapper.vm as any;
    expect(vm.editMcp).toHaveLength(1);
    expect(vm.editMcp[0].tools).toEqual(['tool1', 'tool2']);

    // Changer de serveur
    (selectEl as HTMLSelectElement).value = 'server-b';
    selectEl.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    expect(vm.editMcp[0].tools).toEqual([]);

    // Relancer change pour trigger le pattern test
    selectEl.value = 'server-a';
    selectEl.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));
    selectEl.value = 'server-b';
    selectEl.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise(r => setTimeout(r, 10));

    // Cliquer Sauvegarder du dialog
    const saveBtn = dialog!.querySelector<HTMLButtonElement>('.btn-success')!;
    await saveBtn.click();
    await new Promise(r => setTimeout(r, 50));

    expect(putMcpData?.server).toBe('server-b');
    expect(putMcpData?.tools).toEqual([]);

    expect(vm.editModalOpen).toBe(false);
  });

  // ── Test 7 : Supprimer ─────────────────────────────────────────────────────
  it('Supprimer → DELETE /api/v1/toolsets/{id} puis re-fetch', async () => {
    let fetchCount = 0;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse({
            items: [{ id: 1, name: 'to-delete', local_tools: ['git'], mcp: [] }],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          });
        }
        return jsonResponse({ items: [], page: 1, per_page: 100, total_items: 0, total_pages: 0 });
      }
      if (method === 'DELETE' && url.startsWith('/api/v1/toolsets/')) {
        return new Response(null, { status: 204 });
      }
      if (url === '/api/local-tools') {
        return jsonResponse([]);
      }
      if (url === '/api/v1/mcp-servers') {
        return jsonResponse({ items: [], page: 1, per_page: 100, total_items: 0, total_pages: 0 });
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

  // ── Test 8 : erreur options → message visible sans modale ──────────────────
  it('erreur GET options → message affiché dans le corps principal', async () => {
    // Corps JSON `{error: ...}` — c'est le seul format que client.ts sait extraire
    // (cf. test voisin ci-dessous) ; un corps texte brut sans Content-Type JSON
    // retombe toujours sur "HTTP {status}" côté client, comportement établi.
    mockFetch((url) => {
      if (url === '/api/v1/toolsets') {
        return jsonResponse({
          items: [{
            id: 1,
            name: 'default',
            description: 'Outils par défaut',
            prompt: null,
            local_tools: ['git'],
            mcp: [],
          }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (url === '/api/local-tools') {
        return new Response(JSON.stringify({ error: 'Connection refused' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (url === '/api/v1/mcp-servers') {
        // Retourne JSON pour que l'erreur ait un message lisible, sinon l'API
        // client ne renvoie que "HTTP 500" (sans corps JSON détecté).
        return new Response(JSON.stringify({ error: 'Connection refused' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Le message d'erreur des options est dans le corps principal, visible sans modale.
    expect(wrapper.text()).toContain('Connection refused');
  });

  // ── Test 9 : erreur options → message visible avec modale d'édition ───
  it('erreur GET options → message visible avec modale édition ouverte', async () => {
    let fetchCount = 0;
    mockFetch((url, init) => {
      const method = String(init?.method ?? 'GET').toUpperCase();
      if (method === 'GET' && url === '/api/v1/toolsets') {
        fetchCount++;
        if (fetchCount === 1) {
          return jsonResponse({
            items: [{
              id: 1,
              name: 'default',
              description: 'old-desc',
              prompt: null,
              local_tools: ['git'],
              mcp: [],
            }],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          });
        }
        return jsonResponse({
          items: [{
            id: 1,
            name: 'default',
            description: 'updated',
            prompt: null,
            local_tools: ['git'],
            mcp: [],
          }],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        });
      }
      if (method === 'PUT' && url.startsWith('/api/v1/toolsets/')) {
        return jsonResponse({ id: 1, name: 'default' });
      }
      if (url === '/api/local-tools') {
        return new Response(JSON.stringify({ error: 'Connection refused' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (url === '/api/v1/mcp-servers') {
        return new Response(JSON.stringify({ error: 'Connection refused' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return undefined;
    });

    const wrapper = mount(ToolsetsScreen);
    await new Promise(r => setTimeout(r, 50));

    // Le message d'erreur est dans le corps principal.
    expect(wrapper.text()).toContain('Connection refused');

    // Ouvrir la modale d'édition → le message reste visible.
    await wrapper.find('.btn-edit').trigger('click');
    await new Promise(r => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Connection refused');
  });
});