import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import McpServersScreen from './McpServersScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('McpServersScreen', () => {
  it('affiche noms, types, urls quand GET renvoie 2 serveurs MCP', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            id: 'aaaa1111-bbbb-cccc-dddd-eeee11112222',
            name: 'git-server',
            server_type: 'sse',
            url: 'https://git.example.com/mcp',
          },
          {
            id: 'bbbb2222-cccc-dddd-eeee-ffff33334444',
            name: 'http-server',
            server_type: 'http-streamable',
            url: 'https://http.example.com/mcp',
          },
        ]),
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

  it('remplir le formulaire de création + "Créer" appelle POST avec le corps attendu puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u === '/api/mcp-servers') {
        const body = JSON.parse(String((init as RequestInit)?.body));
        expect(body).toEqual({
          name: 'new-server',
          server_type: 'sse',
          url: 'https://new.example.com/mcp',
        });
        return new Response(
          JSON.stringify({
            id: 'new1111-2222-3333-4444-555566667777',
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

      if (method === 'GET' && u === '/api/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{
              id: 'ex00001',
              name: 'existing',
              server_type: 'sse',
              url: 'https://existing.example.com/mcp',
            }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([
            {
              id: 'ex00001',
              name: 'existing',
              server_type: 'sse',
              url: 'https://existing.example.com/mcp',
            },
            {
              id: 'new1111-2222-3333-4444-555566667777',
              name: 'new-server',
              server_type: 'sse',
              url: 'https://new.example.com/mcp',
            },
          ]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const nameInput = wrapper.find<any>('input[aria-label="Nom du serveur"]');
    const serverTypeSelect = wrapper.find<any>('select[aria-label="Type de serveur"]');
    const urlInput = wrapper.find<any>('input[aria-label="URL"]');

    await nameInput.setValue('new-server');
    await serverTypeSelect.setValue('sse');
    await urlInput.setValue('https://new.example.com/mcp');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-server');
  });

  it('cliquer "Modifier" charge les valeurs ; "Sauvegarder" appelle PUT avec corps update puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    let capturedPutBody: unknown = null;
    let capturedPutUrl = '';
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.startsWith('/api/mcp-servers/')) {
        capturedPutUrl = u;
        capturedPutBody = JSON.parse(String((init as RequestInit)?.body ?? '{}'));
        return new Response(
          JSON.stringify({
            id: 'edit-me-id',
            name: 'updated-name',
            server_type: 'http-streamable',
            url: 'https://updated.example.com/mcp',
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      if (method === 'GET' && u === '/api/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{
              id: 'edit-me-id',
              name: 'old-name',
              server_type: 'sse',
              url: 'https://old.example.com/mcp',
            }]),
            { status: 200, headers: { 'content-type': 'application/json' } },
          );
        }
        return new Response(
          JSON.stringify([{
            id: 'edit-me-id',
            name: 'updated-name',
            server_type: 'http-streamable',
            url: 'https://updated.example.com/mcp',
          }]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();

    expect(wrapper.text()).toContain('Modifier : old-name');
    const editForm = wrapper.findAll<any>('.form-card');

    const editNameInput = editForm[1].find<HTMLInputElement>(
      'input[aria-label="Nom du serveur"]',
    );
    expect(editNameInput!.element.value).toBe('old-name');

    await editNameInput!.setValue('updated-name');

    const editServerTypeSelect = editForm[1].find<any>('select[aria-label="Type de serveur"]');
    await editServerTypeSelect.setValue('http-streamable');

    const editUrlInput = editForm[1].find<HTMLInputElement>(
      'input[aria-label="URL"]',
    );
    await editUrlInput!.setValue('https://updated.example.com/mcp');

    const saveBtns = wrapper.findAll('.btn-success');
    await saveBtns[0].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(capturedPutUrl).toBe('/api/mcp-servers/edit-me-id');
    expect(capturedPutBody).toEqual({
      name: 'updated-name',
      server_type: 'http-streamable',
      url: 'https://updated.example.com/mcp',
    });
  });

  it('cliquer "Supprimer" → DELETE /{id} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/mcp-servers/to-delete-id')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/mcp-servers') {
        fetchCount++;
        if (fetchCount === 1) {
          return new Response(
            JSON.stringify([{
              id: 'to-delete-id',
              name: 'to-delete',
              server_type: 'sse',
              url: 'https://todelete.example.com/mcp',
            }]),
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

    const wrapper = mount(McpServersScreen);
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/mcp-servers/to-delete-id'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun serveur MCP');
  });
});