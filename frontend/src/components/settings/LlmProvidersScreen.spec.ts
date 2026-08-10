import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import LlmProvidersScreen from './LlmProvidersScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('LlmProvidersScreen', () => {
  it('affiche noms, types, endpoints et badge défaut quand GET renvoie 2 fournisseurs', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
          {
            id: 'aaa',
            name: 'ollama-local',
            provider_type: 'ollama',
            endpoint: 'http://localhost:11434',
            api_key: null,
            is_default: true,
            available_models: null,
          },
          {
            id: 'bbb',
            name: 'openai-proxy',
            provider_type: 'openai-compatible',
            endpoint: 'https://openai.example.com/v1',
            api_key: 'sk-test',
            is_default: false,
            available_models: ['gpt-4'],
          },
        ]),
    } as unknown as Response);

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('ollama-local');
    expect(wrapper.text()).toContain('openai-proxy');
    expect(wrapper.text()).toContain('ollama');
    expect(wrapper.text()).toContain('openai-compatible');
    expect(wrapper.text()).toContain('http://localhost:11434');
    expect(wrapper.text()).toContain('https://openai.example.com/v1');
    expect(wrapper.text()).toContain('Défaut');
  });

  it('remplir le formulaire de création + "Créer" appelle POST avec corps attendu puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          // premier GET au montage
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () => Promise.resolve([]),
          } as unknown as Response;
        }
        // re-fetch après CREATE → retourne les données avec le nouveau
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                id: 'new-id',
                name: 'new-provider',
                provider_type: 'ollama',
                endpoint: 'http://localhost:11434',
                api_key: null,
                is_default: false,
                available_models: null,
              },
            ]),
        } as unknown as Response;
      }

      if (method === 'POST' && u === '/api/llm-providers') {
        const body = JSON.parse((init?.body as string | undefined) ?? '{}');
        expect(body).toEqual({
          name: 'new-provider',
          provider_type: 'ollama',
          endpoint: 'http://localhost:11434',
        });
        return {
          ok: true,
          status: 201,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              id: 'new-id',
              name: 'new-provider',
              provider_type: 'ollama',
              endpoint: 'http://localhost:11434',
              api_key: null,
              is_default: false,
              available_models: null,
            }),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const inputs = wrapper.findAll('input');
    const nameInput = inputs[0] as any;
    const providerTypeSelect = wrapper.find('select') as any;
    const endpointInput = inputs[1] as any;
    const apiKeyInput = inputs[2] as any;

    await nameInput.setValue('new-provider');
    await providerTypeSelect.setValue('ollama');
    await endpointInput.setValue('http://localhost:11434');
    await apiKeyInput.setValue('');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-provider');
  });

  it('cliquer "Modifier" charge les valeurs ; "Sauvegarder" appelle PUT avec corps update puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.includes('/api/llm-providers/aaa')) {
        const body = JSON.parse((init?.body as string | undefined) ?? '{}');
        expect(body).toEqual({
          name: 'updated-ollama',
          provider_type: 'ollama',
          endpoint: 'http://localhost:11435',
        });
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              id: 'aaa',
              name: 'updated-ollama',
              provider_type: 'ollama',
              endpoint: 'http://localhost:11435',
              api_key: null,
              is_default: true,
              available_models: null,
            }),
        } as unknown as Response;
      }

      if (method === 'GET' && u === '/api/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  id: 'aaa',
                  name: 'ollama-local',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: true,
                  available_models: null,
                },
              ]),
          } as unknown as Response;
        }
        // re-fetch après SAVE
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                id: 'aaa',
                name: 'updated-ollama',
                provider_type: 'ollama',
                endpoint: 'http://localhost:11435',
                api_key: null,
                is_default: true,
                available_models: null,
              },
            ]),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const editBtns = wrapper.findAll('.btn-edit');
    await editBtns[0].trigger('click');
    await wrapper.vm.$nextTick();

    // Vérifier que le formulaire d'édition est affiché avec les pré-remplis
    expect(wrapper.text()).toContain('Modifier : ollama-local');
    const editForm = wrapper.findAll('.form-card');
    const editInputs = editForm[1].findAll('input') as any;
    expect(editInputs[0].element.value).toBe('ollama-local');
    expect(editInputs[1].element.value).toBe('http://localhost:11434');

    // Remplir les valeurs modifiées
    await editInputs[0].setValue('updated-ollama');
    await editInputs[1].setValue('http://localhost:11435');

    const saveBtns = wrapper.findAll('.btn-success');
    await saveBtns[0].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('updated-ollama');
  });

  it('cliquer "Tester" → POST /{id}/test, et les modèles retournés s\'affichent', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u.includes('/api/llm-providers/aaa/test')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({ models: ['llama3', 'mistral', 'gemma'] }),
        } as unknown as Response;
      }

      if (method === 'GET' && u === '/api/llm-providers') {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                id: 'aaa',
                name: 'ollama-local',
                provider_type: 'ollama',
                endpoint: 'http://localhost:11434',
                api_key: null,
                is_default: false,
                available_models: null,
              },
            ]),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const testBtn = wrapper.find('.btn-test');
    await testBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('llama3');
    expect(wrapper.text()).toContain('mistral');
    expect(wrapper.text()).toContain('gemma');
  });

  it('cliquer "Défaut" → PUT /{id}/default, puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.includes('/api/llm-providers/bbb/default')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              id: 'bbb',
              name: 'openai-proxy',
              provider_type: 'openai-compatible',
              endpoint: 'https://openai.example.com/v1',
              api_key: 'sk-test',
              is_default: true,
              available_models: ['gpt-4'],
            }),
        } as unknown as Response;
      }

      if (method === 'GET' && u === '/api/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  id: 'aaa',
                  name: 'ollama-local',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: true,
                  available_models: null,
                },
                {
                  id: 'bbb',
                  name: 'openai-proxy',
                  provider_type: 'openai-compatible',
                  endpoint: 'https://openai.example.com/v1',
                  api_key: 'sk-test',
                  is_default: false,
                  available_models: ['gpt-4'],
                },
              ]),
          } as unknown as Response;
        }
        // re-fetch après SET_DEFAULT — bbb est maintenant défaut
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                id: 'aaa',
                name: 'ollama-local',
                provider_type: 'ollama',
                endpoint: 'http://localhost:11434',
                api_key: null,
                is_default: false,
                available_models: null,
              },
              {
                id: 'bbb',
                name: 'openai-proxy',
                provider_type: 'openai-compatible',
                endpoint: 'https://openai.example.com/v1',
                api_key: 'sk-test',
                is_default: true,
                available_models: ['gpt-4'],
              },
            ]),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('openai-proxy');

    const defaultBtns = wrapper.findAll('.btn-default');
    // Le deuxième fournisseur (bbb) n'est pas défaut → cliquer sur le 2ème bouton
    await defaultBtns[1].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Après le re-fetch, le badge "Défaut" devrait apparaître sur bbb
    expect(wrapper.text()).toContain('Défaut');
  });

  it('cliquer "Supprimer" → DELETE /{id} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/llm-providers/aaa')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve([
                {
                  id: 'aaa',
                  name: 'to-delete',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: false,
                  available_models: null,
                },
              ]),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () => Promise.resolve([]),
        } as unknown as Response;
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteBtn = wrapper.find('.btn-delete');
    await deleteBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const deleteCalls = fetchSpy.mock.calls.filter(
      ([url]) => String(url).includes('/api/llm-providers/aaa'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun fournisseur');
  });
});