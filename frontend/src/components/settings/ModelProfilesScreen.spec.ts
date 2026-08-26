import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import ModelProfilesScreen from './ModelProfilesScreen.vue';

describe('ModelProfilesScreen', () => {
  let fetchCalls: string[];
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    fetchCalls = [];
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockReset();
    // Nettoyer le body après chaque test (téléport reka-ui)
    const dialogs = document.body.querySelectorAll('[role="dialog"]');
    dialogs.forEach((d) => d.remove());
  });

  // Helpers for the mock setup
  const providersResponse = [
    {
      id: 1,
      name: 'anthropic',
      provider_type: 'openai_compatible',
      endpoint: 'https://api.anthropic.com/v1',
      available_models: ['claude-sonnet-4-20250514', 'claude-haiku-4-20250514'],
      is_default: false,
    },
    {
      id: 2,
      name: 'openai',
      provider_type: 'openai_compatible',
      endpoint: 'https://api.openai.com/v1',
      available_models: [],
      is_default: false,
    },
  ];

  it('GET /api/v1/model-profiles renvoie 2 → affiche noms, providers, modèles', async () => {
    const profiles = [
      {
        id: 1,
        name: 'chat-moderate',
        provider_id: 1,
        model: 'claude-sonnet-4-20250514',
        temperature: 0.4,
        max_tokens: 4096,
      },
      {
        id: 2,
        name: 'chat-fast',
        provider_id: 2,
        model: 'gpt-4o-mini',
      },
    ];

    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      fetchCalls.push(`GET ${urlStr}`);
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: profiles, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('chat-moderate');
    expect(wrapper.text()).toContain('anthropic');
    expect(wrapper.text()).toContain('claude-sonnet-4-20250514');
    expect(wrapper.text()).toContain('chat-fast');
    expect(wrapper.text()).toContain('openai');
    expect(wrapper.text()).toContain('gpt-4o-mini');
    expect(wrapper.text()).toContain('0.4');
    expect(wrapper.text()).toContain('4096');
    expect(wrapper.text()).toContain('—');
  });

  it('select Provider alimenté par GET /api/v1/llm-providers', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      fetchCalls.push(`GET ${url}`);
      const urlStr = url as string;
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Ouvrir la modale de création
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();

    const providerSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    expect(providerSelect).toBeTruthy();
    const options = providerSelect!.options;
    expect(options.length).toBe(3); // vide + anthropic + openai

    // Choisir anthropic (id=1) → select Modèle contient 2 modèles
    providerSelect!.value = '1';
    providerSelect!.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Modèle"]');
    const modelOptions = modelSelect!.options;
    expect(modelOptions.length).toBe(3);
    expect(modelOptions[1].textContent).toBe('claude-sonnet-4-20250514');
    expect(modelOptions[2].textContent).toBe('claude-haiku-4-20250514');
    expect(modelSelect!.value).toBe('');

    expect(dialog!.textContent).not.toContain('Aucun modèle disponible');
  });

  it('POST corps avec provider_id : { name, provider_id, model, ... }', async () => {
    let created = false;
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const urlStr = url as string;
      const method = (init as RequestInit)?.method ?? 'GET';

      if (method === 'GET' && urlStr.endsWith('/api/v1/model-profiles')) {
        fetchCalls.push(`GET ${urlStr}`);
        if (created) {
          return new Response(JSON.stringify({
            items: [{
              id: 3,
              name: 'new-profile',
              provider_id: 1,
              model: 'claude-sonnet-4-20250514',
              temperature: 0.7,
              max_tokens: null,
            }], page: 1, per_page: 100, total_items: 1, total_pages: 1,
          }), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          });
        }
        return new Response(JSON.stringify({
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'POST' && urlStr.endsWith('/api/v1/model-profiles')) {
        fetchCalls.push('POST /api/v1/model-profiles');
        const body = JSON.parse((init as RequestInit)?.body as string ?? '{}');
        expect(body).toEqual({
          name: 'new-profile',
          provider_id: 1,
          model: 'claude-sonnet-4-20250514',
          temperature: 0.7,
        });
        created = true;
        return new Response(JSON.stringify({
          id: 3,
          name: 'new-profile',
          provider_id: 1,
          model: 'claude-sonnet-4-20250514',
          temperature: 0.7,
          max_tokens: null,
        }), {
          status: 201,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Ouvrir la modale de création
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const dialog = document.querySelector('[role="dialog"]')!;

    // Remplir les champs du dialog
    const inputs = dialog.querySelectorAll('input');
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(inputs[0], 'new-profile');

    const providerSelect = dialog.querySelector('select[aria-label="Provider"]') as HTMLSelectElement;
    providerSelect.value = '1';
    providerSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = dialog.querySelector('select[aria-label="Modèle"]') as HTMLSelectElement;
    modelSelect.value = 'claude-sonnet-4-20250514';
    modelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // température (2ᵉ input, index 1)
    setInput(inputs[1], '0.7');

    // Cliquer "Créer" du dialog
    const dialogCreateBtn = dialog.querySelector('.btn-create') as HTMLElement;
    await dialogCreateBtn.click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 100));

    expect(created).toBe(true);
    // Si le dialog est toujours dans le body (data-state="closed"),
    // vérifier l'état du composant
    const dialogStillExists = document.querySelector('[role="dialog"]');
    if (dialogStillExists) {
      expect((wrapper.vm as any).createModalOpen).toBe(false);
    }
    expect(wrapper.text()).toContain('new-profile');
  });

  it('état vide : provider choisi sans modèles → message affiché', async () => {
    const providersNoModels = [
      {
        id: 1,
        name: 'anthropic',
        provider_type: 'openai_compatible',
        endpoint: 'https://api.anthropic.com/v1',
        available_models: ['claude-sonnet-4-20250514'],
        is_default: false,
      },
      {
        id: 2,
        name: 'custom-provider',
        provider_type: 'openai_compatible',
        endpoint: 'https://custom.example.com/v1',
        available_models: [],
        is_default: false,
      },
    ];
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersNoModels, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Ouvrir la modale de création
    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const dialog = document.querySelector('[role="dialog"]')!;

    const providerSelect = dialog.querySelector('select[aria-label="Provider"]') as HTMLSelectElement;
    providerSelect.value = '2';
    providerSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Vérifier dans le dialog téléporté
    expect(dialog.textContent).toContain('Aucun modèle disponible');
    expect(dialog.textContent).toContain('lancez un test sur ce provider');
  });

  it('edit — chargement + PUT par id numérique', async () => {
    const originalProfile = {
      id: 1,
      name: 'aaa',
      provider_id: 1,
      model: 'original-model',
      temperature: 0.3,
      max_tokens: 4096,
    };
    const updatedProfile = {
      id: 1,
      name: 'aaa',
      provider_id: 1,
      model: 'updated-model',
      temperature: 0.5,
      max_tokens: 8192,
    };
    const providers = providersResponse;

    // Sequence of fetch responses (no closures — each call consumes the next one)
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy
      // 1: GET /api/v1/model-profiles → mount → [originalProfile]
      .mockResolvedValueOnce(new Response(JSON.stringify({
        items: [originalProfile], page: 1, per_page: 100, total_items: 1, total_pages: 1,
      }), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }))
      // 2: GET /api/v1/llm-providers → mount → providers
      .mockResolvedValueOnce(new Response(JSON.stringify({
        items: providers, page: 1, per_page: 100, total_items: 2, total_pages: 1,
      }), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }))
      // 3: PUT /api/v1/model-profiles/1 → save → UPDATED (assert body in mock)
      .mockImplementationOnce(async (_url: unknown, init: unknown) => {
        const bodyStr = (init as RequestInit)?.body as string;
        const body = JSON.parse(bodyStr ?? '{}');
        // PUT body contains ALL fields from edit form:
        // editProviderId=1, editModel='updated-model' (set in test),
        // editTemperature='0.3' (from startEdit), editMaxTokens='4096' (from startEdit)
        expect(body).toEqual({
          provider_id: 1,
          model: 'updated-model',
          temperature: 0.3,
          max_tokens: 4096,
        });
        return new Response(JSON.stringify(updatedProfile), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      })
      // 4: GET /api/v1/model-profiles → fetchProfiles after save → [new data]
      ;

    // After the chain is consumed, set a fallback that handles ALL URLs
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const method = (init as RequestInit)?.method ?? 'GET';
      const urlStr = url as string;
      if (method === 'GET' && urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [updatedProfile], page: 1, per_page: 100, total_items: 1, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providers, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'DELETE') {
        return new Response('', { status: 204, headers: { 'Content-Type': 'text/plain' } });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('original-model');

    // Ouvrir l'édition
    await (wrapper.find('.btn-edit')).trigger('click');
    // Allow watcher to populate editAvailableModels
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));

    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog!.textContent).toContain('Modifier : aaa');

    // Provider pré-rempli (value=1 pour anthropic)
    const editProviderSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    expect(editProviderSelect?.value).toBe('1');

    // editAvailableModels peuplé par watcher
    const vm = wrapper.vm as any;
    expect(vm.editAvailableModels.length).toBe(2); // claude-sonnet + claude-haiku

    // Model select options dans le dialog
    const editModelSelect = dialog!.querySelector<HTMLSelectElement>('select[aria-label="Modèle"]');
    expect(editModelSelect?.options.length).toBe(3); // vide + 2 modèles

    // Mettre à jour editModel directement
    vm.editModel = 'updated-model';
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));
    await new Promise((r) => setTimeout(r, 50));
    expect(vm.editModel).toBe('updated-model');

    // Sauvegarder
    const saveBtn = dialog!.querySelector<HTMLButtonElement>('.btn-success')!;
    await saveBtn.click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 200));

    // Le PUT a été appelé (expect dans le mock a vérifié le corps)
    // Le re-fetch après PUT retourne updatedProfile
    expect(wrapper.text()).toContain('updated-model');
  });

  it('edit — changer le provider met à jour editAvailableModels et reset editModel', async () => {
    const profile = {
      id: 1,
      name: 'aaa',
      provider_id: 1,
      model: 'original-model',
      temperature: 0.3,
      max_tokens: 4096,
    };
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [profile], page: 1, per_page: 100, total_items: 1, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    await (wrapper.find('.btn-edit')).trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));

    const dialog = document.querySelector('[role="dialog"]')!;
    const editProviderSelect = dialog.querySelector('select[aria-label="Provider"]') as HTMLSelectElement;

    expect(editProviderSelect.value).toBe('1');

    // Changer vers openai (id=2, sans modèles)
    editProviderSelect.value = '2';
    editProviderSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const vm = wrapper.vm as any;
    expect(vm.editAvailableModels).toEqual([]);
    expect(editProviderSelect.value).toBe('2');

    const editModelSelect = dialog.querySelector('select[aria-label="Modèle"]') as HTMLSelectElement;
    expect(editModelSelect.value).toBe('');
    expect(dialog.textContent).toContain('Aucun modèle disponible');
  });

  it('erreur GET /api/v1/llm-providers → message affiché dans le corps principal', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({ error: 'Network error' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Le message est rendu dans le corps principal (avant le bouton Créer), visible sans ouvrir de modale.
    expect(wrapper.text()).toContain('Network error');
  });

  it('options avancées : ligne clé/valeur → POST inclut options avec la valeur parsée en JSON', async () => {
    let created = false;
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const urlStr = url as string;
      const method = (init as RequestInit)?.method ?? 'GET';

      if (method === 'GET' && urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify(created ? {
          items: [{
            id: 3,
            name: 'new-profile',
            provider_id: 1,
            model: 'claude-sonnet-4-20250514',
            options: { top_p: 0.9 },
          }], page: 1, per_page: 100, total_items: 1, total_pages: 1,
        } : {
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'POST' && urlStr.endsWith('/api/v1/model-profiles')) {
        const body = JSON.parse((init as RequestInit)?.body as string ?? '{}');
        expect(body.options).toEqual({ top_p: 0.9 });
        created = true;
        return new Response(JSON.stringify({
          id: 3,
          name: 'new-profile',
          provider_id: 1,
          model: 'claude-sonnet-4-20250514',
          options: { top_p: 0.9 },
        }), {
          status: 201,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    await wrapper.find('.btn-create').trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const dialog = document.querySelector('[role="dialog"]')!;
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(dialog.querySelectorAll('input')[0], 'new-profile');

    const providerSelect = dialog.querySelector('select[aria-label="Provider"]') as HTMLSelectElement;
    providerSelect.value = '1';
    providerSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = dialog.querySelector('select[aria-label="Modèle"]') as HTMLSelectElement;
    modelSelect.value = 'claude-sonnet-4-20250514';
    modelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    await (dialog.querySelector('.option-add') as HTMLElement).click();
    await wrapper.vm.$nextTick();
    setInput(dialog.querySelector('[aria-label="Option 1 clé"]')!, 'top_p');
    setInput(dialog.querySelector('[aria-label="Option 1 valeur"]')!, '0.9');

    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 100));

    expect(created).toBe(true);
    expect(wrapper.text()).toContain('new-profile');
  });

  it('options avancées : ligne sans clé → POST n\'inclut pas options', async () => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    let posted = false;
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const urlStr = url as string;
      const method = (init as RequestInit)?.method ?? 'GET';
      if (method === 'GET' && urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [], page: 1, per_page: 100, total_items: 0, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'POST' && urlStr.endsWith('/api/v1/model-profiles')) {
        const body = JSON.parse((init as RequestInit)?.body as string ?? '{}');
        expect(body.options).toBeUndefined();
        posted = true;
        return new Response(JSON.stringify({ id: 3, name: 'x', provider_id: 1, model: 'm' }), {
          status: 201, headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));
    await wrapper.find('.btn-create').trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const dialog = document.querySelector('[role="dialog"]')!;
    const setInput = (el: Element, val: string) => {
      (el as HTMLInputElement).value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput(dialog.querySelectorAll('input')[0], 'x');
    const providerSelect = dialog.querySelector('select[aria-label="Provider"]') as HTMLSelectElement;
    providerSelect.value = '1';
    providerSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));
    const modelSelect = dialog.querySelector('select[aria-label="Modèle"]') as HTMLSelectElement;
    modelSelect.value = 'claude-sonnet-4-20250514';
    modelSelect.dispatchEvent(new Event('change', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 10));

    // Ligne ajoutée mais laissée vide (pas de clé) — ne doit pas produire `options`.
    await (dialog.querySelector('.option-add') as HTMLElement).click();
    await wrapper.vm.$nextTick();

    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 100));

    expect(posted).toBe(true);
  });

  it('edit — options existantes pré-remplissent l\'éditeur clé/valeur', async () => {
    const profile = {
      id: 1,
      name: 'aaa',
      provider_id: 1,
      model: 'original-model',
      options: { top_p: 0.9, thinking_mode: 'enabled' },
    };
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/v1/model-profiles')) {
        return new Response(JSON.stringify({
          items: [profile], page: 1, per_page: 100, total_items: 1, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/v1/llm-providers')) {
        return new Response(JSON.stringify({
          items: providersResponse, page: 1, per_page: 100, total_items: 2, total_pages: 1,
        }), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    await wrapper.find('.btn-edit').trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));

    const vm = wrapper.vm as any;
    expect(vm.editOptions).toEqual([
      { key: 'top_p', value: '0.9' },
      { key: 'thinking_mode', value: 'enabled' },
    ]);
  });

  it('erreur GET /api/v1/llm-providers → message visible même avec modale d\'édition ouverte', async () => {
    const profile = {
      id: 1,
      name: 'aaa',
      provider_id: 1,
      model: 'model-1',
      temperature: 0.5,
      max_tokens: 4096,
    };
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy
      .mockResolvedValueOnce(new Response(JSON.stringify({
        items: [profile], page: 1, per_page: 100, total_items: 1, total_pages: 1,
      }), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: 'Network error' }), {
        status: 502, headers: { 'Content-Type': 'application/json' },
      }));

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    // Le message d'erreur des providers est dans le corps principal.
    expect(wrapper.text()).toContain('Network error');

    // Ouvrir la modale d'édition → le message reste visible (dans le corps principal).
    await (wrapper.find('.btn-edit')).trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Network error');
  });
});