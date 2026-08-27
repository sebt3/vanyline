import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import LlmProvidersScreen from './LlmProvidersScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
  // Nettoyer le body après chaque test (téléport reka-ui)
  const dialogs = document.body.querySelectorAll('[role="dialog"]');
  dialogs.forEach((d) => d.remove());
});

describe('LlmProvidersScreen', () => {
  it('affiche noms, types, endpoints et badge défaut quand GET renvoie 2 fournisseurs', async () => {
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
              provider_type: 'ollama',
              endpoint: 'http://localhost:11434',
              api_key: null,
              is_default: true,
              available_models: null,
            },
            {
              id: 2,
              name: 'openai-proxy',
              provider_type: 'openai-compatible',
              endpoint: 'https://openai.example.com/v1',
              api_key: 'sk-test',
              is_default: false,
              available_models: ['gpt-4'],
            },
          ],
          page: 1,
          per_page: 100,
          total_items: 2,
          total_pages: 1,
        }),
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

  it('création en modale : dialog apparaît → remplir → créer → dialog fermé', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/v1/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve({
                items: [],
                page: 1,
                per_page: 100,
                total_items: 0,
                total_pages: 1,
              }),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              items: [
                {
                  id: 3,
                  name: 'new-provider',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: false,
                  available_models: null,
                },
              ],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
        } as unknown as Response;
      }

      if (method === 'POST' && u === '/api/v1/llm-providers') {
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
              id: 3,
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

    // Cliquer "Créer un fournisseur" → dialog apparaît
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="dialog"]')).toBeTruthy();

    const dialog = document.querySelector('[role="dialog"]')!;

    // Remplir les champs
    const inputs = dialog.querySelectorAll('input');
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(inputs[0], 'new-provider');
    setInput(inputs[1], 'http://localhost:11434');
    // api_key reste vide

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create');
    await (dialogCreateBtn as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();

    // Nouvelle donnée apparaît
    expect(wrapper.text()).toContain('new-provider');
  });

  it('cliquer "Modifier" ouvre la modale avec valeurs pré-remplies', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'GET' && u === '/api/v1/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve({
                items: [
                  {
                    id: 1,
                    name: 'ollama-local',
                    provider_type: 'ollama',
                    endpoint: 'http://localhost:11434',
                    api_key: null,
                    is_default: true,
                    available_models: null,
                  },
                ],
                page: 1,
                per_page: 100,
                total_items: 1,
                total_pages: 1,
              }),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              items: [
                {
                  id: 1,
                  name: 'updated-ollama',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11435',
                  api_key: null,
                  is_default: true,
                  available_models: null,
                },
              ],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
        } as unknown as Response;
      }

      if (method === 'PUT' && u.includes('/api/v1/llm-providers/1')) {
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
          json: () => ({
            id: 1,
            name: 'updated-ollama',
            provider_type: 'ollama',
            endpoint: 'http://localhost:11435',
            api_key: null,
            is_default: true,
            available_models: null,
          }),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Cliquer "Modifier" sur une ligne
    const editBtn = wrapper.find('.btn-edit');
    await editBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Le dialog existe avec le titre pré-rempli
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog?.textContent).toContain('Modifier : ollama-local');

    // Les champs sont pré-remplis
    const inputs = dialog!.querySelectorAll('input');
    expect((inputs[0] as HTMLInputElement).value).toBe('ollama-local');
    expect((inputs[1] as HTMLInputElement).value).toBe('http://localhost:11434');

    // Remplir les valeurs modifiées
    (inputs[0] as HTMLInputElement).value = 'updated-ollama';
    (inputs[0] as HTMLInputElement).dispatchEvent(new Event('input', { bubbles: true }));
    (inputs[1] as HTMLInputElement).value = 'http://localhost:11435';
    (inputs[1] as HTMLInputElement).dispatchEvent(new Event('input', { bubbles: true }));

    // Cliquer "Sauvegarder"
    (dialog!.querySelector('.btn-success') as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Dialog fermé
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();

    // Re-fetch → valeurs mises à jour
    expect(wrapper.text()).toContain('updated-ollama');
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
              provider_type: 'ollama',
              endpoint: 'http://localhost:11434',
              api_key: null,
              is_default: true,
              available_models: null,
            },
          ],
          page: 1,
          per_page: 100,
          total_items: 1,
          total_pages: 1,
        }),
    } as unknown as Response);

    const wrapper = mount(LlmProvidersScreen);

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

  it('cliquer "Tester" → POST /api/v1/llm-providers/{id}/test, et les modèles retournés s\'affichent', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'POST' && u.includes('/api/v1/llm-providers/1/test')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({ models: ['llama3', 'mistral', 'gemma'] }),
        } as unknown as Response;
      }

      if (method === 'GET' && u === '/api/v1/llm-providers') {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              items: [
                {
                  id: 1,
                  name: 'ollama-local',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: false,
                  available_models: null,
                },
              ],
              page: 1,
              per_page: 100,
              total_items: 1,
              total_pages: 1,
            }),
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

  it('cliquer "Défaut" → PUT /api/v1/llm-providers/{id}/default, puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);

      if (method === 'PUT' && u.includes('/api/v1/llm-providers/2/default')) {
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              id: 2,
              name: 'openai-proxy',
              provider_type: 'openai-compatible',
              endpoint: 'https://openai.example.com/v1',
              api_key: 'sk-test',
              is_default: true,
              available_models: ['gpt-4'],
            }),
        } as unknown as Response;
      }

      if (method === 'GET' && u === '/api/v1/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve({
                items: [
                  {
                    id: 1,
                    name: 'ollama-local',
                    provider_type: 'ollama',
                    endpoint: 'http://localhost:11434',
                    api_key: null,
                    is_default: true,
                    available_models: null,
                  },
                  {
                    id: 2,
                    name: 'openai-proxy',
                    provider_type: 'openai-compatible',
                    endpoint: 'https://openai.example.com/v1',
                    api_key: 'sk-test',
                    is_default: false,
                    available_models: ['gpt-4'],
                  },
                ],
                page: 1,
                per_page: 100,
                total_items: 2,
                total_pages: 1,
              }),
          } as unknown as Response;
        }
        // re-fetch après SET_DEFAULT — bbb est maintenant défaut
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              items: [
                {
                  id: 1,
                  name: 'ollama-local',
                  provider_type: 'ollama',
                  endpoint: 'http://localhost:11434',
                  api_key: null,
                  is_default: false,
                  available_models: null,
                },
                {
                  id: 2,
                  name: 'openai-proxy',
                  provider_type: 'openai-compatible',
                  endpoint: 'https://openai.example.com/v1',
                  api_key: 'sk-test',
                  is_default: true,
                  available_models: ['gpt-4'],
                },
              ],
              page: 1,
              per_page: 100,
              total_items: 2,
              total_pages: 1,
            }),
        } as unknown as Response;
      }

      return new Response(null, { status: 500 });
    });

    const wrapper = mount(LlmProvidersScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('openai-proxy');

    const defaultBtns = wrapper.findAll('.btn-default');
    // Le deuxième fournisseur (2) n'est pas défaut → cliquer sur le 2ème bouton
    await defaultBtns[1].trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Après le re-fetch, le badge "Défaut" devrait apparaître sur bbb
    expect(wrapper.text()).toContain('Défaut');
  });

  it('cliquer "Supprimer" → DELETE /api/v1/llm-providers/{id} puis re-fetch', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    let fetchCount = 0;
    fetchSpy.mockImplementation(async (url, init) => {
      const method = (init?.method ?? 'GET') as string;
      const u = String(url);
      if (method === 'DELETE' && u.includes('/api/v1/llm-providers/1')) {
        return new Response(null, { status: 204 });
      }
      if (method === 'GET' && u === '/api/v1/llm-providers') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () =>
              Promise.resolve({
                items: [
                  {
                    id: 1,
                    name: 'to-delete',
                    provider_type: 'ollama',
                    endpoint: 'http://localhost:11434',
                    api_key: null,
                    is_default: false,
                    available_models: null,
                  },
                ],
                page: 1,
                per_page: 100,
                total_items: 1,
                total_pages: 1,
              }),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              items: [],
              page: 1,
              per_page: 100,
              total_items: 0,
              total_pages: 1,
            }),
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
      ([url]) => String(url).includes('/api/v1/llm-providers/1'),
    );
    expect(deleteCalls.length).toBe(1);
    expect(deleteCalls[0][1]).toMatchObject({
      method: 'DELETE',
    });

    expect(wrapper.text()).toContain('Aucun fournisseur');
  });
});