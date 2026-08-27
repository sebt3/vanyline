import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import McpServersScreen from './McpServersScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
});

describe('McpServersScreen', () => {
  it('affiche noms, types, urls quand GET renvoie 2 serveurs MCP', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve({
          items: [
            {
              id: 1,
              name: 'git-server',
              server_type: 'sse',
              url: 'https://git.example.com/mcp',
            },
            {
              id: 2,
              name: 'http-server',
              server_type: 'http-streamable',
              url: 'https://http.example.com/mcp',
            },
          ],
          page: 1,
          per_page: 100,
          total_items: 2,
          total_pages: 1,
        }),
    } as unknown as Response);

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('git-server');
    expect(wrapper.text()).toContain('http-server');
    expect(wrapper.text()).toContain('sse');
    expect(wrapper.text()).toContain('http-streamable');
    expect(wrapper.text()).toContain('https://git.example.com/mcp');
    expect(wrapper.text()).toContain('https://http.example.com/mcp');
  });

  it('création en modale : dialog apparaît → remplir → créer → dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/v1/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify({
              items: [{
                id: 1,
                name: 'existing',
                server_type: 'sse',
                url: 'https://existing.example.com/mcp',
              }],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify({
            items: [
              {
                id: 1,
                name: 'existing',
                server_type: 'sse',
                url: 'https://existing.example.com/mcp',
              },
              {
                id: 3,
                name: 'new-server',
                server_type: 'sse',
                url: 'https://new.example.com/mcp',
              },
            ],
            page: 1,
            per_page: 100,
            total_items: 2,
            total_pages: 1,
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'POST' && u === '/api/v1/mcp-servers') {
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(body).toEqual({
          name: 'new-server',
          server_type: 'sse',
          url: 'https://new.example.com/mcp',
        });
        return new Response(
          JSON.stringify({
            id: 3,
            name: 'new-server',
            server_type: 'sse',
            url: 'https://new.example.com/mcp',
          }),
          {
            status: 201,
            headers: { 'content-type': 'application/json' },
          },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Créer un serveur MCP" → dialog apparaît
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="dialog"]')).toBeTruthy();

    const dialog = document.querySelector('[role="dialog"]')!;

    // Remplir les champs
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(dialog.querySelector('input')!, 'new-server');
    
    const select = dialog.querySelector('select')! as HTMLSelectElement;
    select.value = 'sse';
    select.dispatchEvent(new Event('change', { bubbles: true }));

    const urlInput = dialog.querySelectorAll('input')[1];
    setInput(urlInput, 'https://new.example.com/mcp');

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create') as HTMLElement;
    await dialogCreateBtn.click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();

    // Nouvelle donnée apparaît
    expect(wrapper.text()).toContain('new-server');
  });

  it('cliquer "Modifier" ouvre la modale avec valeurs pré-remplies', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    let capturedPutBody: unknown = null;
    let capturedPutUrl = '';
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.startsWith('/api/v1/mcp-servers/')) {
        capturedPutUrl = u;
        capturedPutBody = JSON.parse(String((init as RequestInit)?.body ?? '{}'));
        return new Response(
          JSON.stringify({
            id: 2,
            name: 'updated-name',
            server_type: 'http-streamable',
            url: 'https://updated.example.com/mcp',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/v1/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify({
              items: [{
                id: 2,
                name: 'old-name',
                server_type: 'sse',
                url: 'https://old.example.com/mcp',
              }],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify({
            items: [{
              id: 2,
              name: 'updated-name',
              server_type: 'http-streamable',
              url: 'https://updated.example.com/mcp',
            }],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Modifier" sur une ligne
    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Le dialog existe
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog?.textContent).toContain('Modifier : old-name');

    // Les champs sont pré-remplis
    const inputs = dialog!.querySelectorAll('input');
    expect((inputs[0] as HTMLInputElement).value).toBe('old-name');

    // Remplir les valeurs modifiées
    (inputs[0] as HTMLInputElement).value = 'updated-name';
    (inputs[0] as HTMLInputElement).dispatchEvent(new Event('input', { bubbles: true }));

    const select = dialog!.querySelector('select') as HTMLSelectElement;
    select.value = 'http-streamable';
    select.dispatchEvent(new Event('change', { bubbles: true }));

    const urlInput = dialog!.querySelectorAll('input')[1];
    (urlInput as HTMLInputElement).value = 'https://updated.example.com/mcp';
    (urlInput as HTMLInputElement).dispatchEvent(new Event('input', { bubbles: true }));

    // Cliquer "Sauvegarder"
    (dialog!.querySelector('.btn-success') as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();

    // Vérifier les appels PUT
    expect(capturedPutUrl).toBe('/api/v1/mcp-servers/2');
    expect(capturedPutBody).toEqual({
      name: 'updated-name',
      server_type: 'http-streamable',
      url: 'https://updated.example.com/mcp',
    });
  });

  it('annuler (bouton Annuler) la modale d\'édition → pas de PUT, dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve({
          items: [
            {
              id: 1,
              name: 'ollama-local',
              server_type: 'sse',
              url: 'https://ollama.example.com/mcp',
            },
          ],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        }),
    } as unknown as Response);

    const wrapper = mount(McpServersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Modifier"
    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="dialog"]')).toBeTruthy();

    // Cliquer "Annuler"
    const dialog = document.querySelector('[role="dialog"]')!;
    const cancelButton = dialog.querySelector('.btn-cancel');
    await (cancelButton as HTMLElement).click();
    await wrapper.vm.$nextTick();

    // Dialog fermé — état du composant
    expect((wrapper.vm as any).editModalOpen).toBe(false);

    // Aucun PUT appelé
    const putCalls = fetchSpy.mock.calls.filter(
      ([_url, init]) => (init?.method ?? 'GET') === 'PUT',
    );
    expect(putCalls.length).toBe(0);
  });

  it('cliquer "Supprimer" → DELETE /api/v1/mcp-servers/{id} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/v1/mcp-servers/3')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/v1/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify({
              items: [{
                id: 3,
                name: 'to-delete',
                server_type: 'sse',
                url: 'https://todelete.example.com/mcp',
              }],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(JSON.stringify({
          items: [],
          page: 1,
          per_page: 100,
          total_items: 0,
          total_pages: 1,
        }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/v1/mcp-servers/3'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun serveur MCP');
  });

  it('"Découvrir" appelle POST /api/v1/mcp-servers/{id}/test et affiche les tools renvoyés', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/v1/mcp-servers') {
        return new Response(
          JSON.stringify({
            items: [
              {
                id: 1,
                name: 'git-server',
                server_type: 'http-streamable',
                url: 'https://git.example.com/mcp',
              },
            ],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }
      if (method === 'POST' && u === '/api/v1/mcp-servers/1/test') {
        return new Response(
          JSON.stringify({ tools: ['read_file', 'write_file', 'list_dir'] }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('jamais testé');

    await wrapper.find('.btn-discover').trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/v1/mcp-servers/1/test',
      expect.objectContaining({ method: 'POST' }),
    );
    expect(wrapper.text()).toContain('3 tools');
    expect(wrapper.text()).not.toContain('jamais testé');
  });

  it('"Découvrir" en échec affiche une erreur par ligne, sans planter', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/v1/mcp-servers') {
        return new Response(
          JSON.stringify({
            items: [
              {
                id: 2,
                name: 'flaky-server',
                server_type: 'sse',
                url: 'https://flaky.example.com/mcp',
              },
            ],
            page: 1,
            per_page: 100,
            total_items: 1,
            total_pages: 1,
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }
      if (method === 'POST' && u === '/api/v1/mcp-servers/2/test') {
        return new Response(
          JSON.stringify({ error: 'VNL-MCP-004: transport SSE non implémenté' }),
          { status: 400, headers: { 'content-type': 'application/json' } },
        );
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    await wrapper.find('.btn-discover').trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('VNL-MCP-004');
    // La ligne reste utilisable — pas de crash du composant.
    expect(wrapper.text()).toContain('flaky-server');
  });
});