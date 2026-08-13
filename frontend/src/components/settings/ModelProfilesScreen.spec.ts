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
  });

  // Helpers for the mock setup
  const providersResponse = [
    {
      name: 'anthropic',
      provider_type: 'openai_compatible',
      endpoint: 'https://api.anthropic.com/v1',
      available_models: ['claude-sonnet-4-20250514', 'claude-haiku-4-20250514'],
      is_default: false,
    },
    {
      name: 'openai',
      provider_type: 'openai_compatible',
      endpoint: 'https://api.openai.com/v1',
      available_models: [],
      is_default: false,
    },
  ];

  it('GET /api/model-profiles renvoie 2 → affiche noms, providers, modèles', async () => {
    const profiles = [
      {
        name: 'chat-moderate',
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514',
        temperature: 0.4,
        max_tokens: 4096,
      },
      {
        name: 'chat-fast',
        provider: 'openai',
        model: 'gpt-4o-mini',
      },
    ];

    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      fetchCalls.push(`GET ${urlStr}`);
      if (urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify(profiles), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providersResponse), {
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

  it('select Provider alimenté par GET /api/llm-providers', async () => {
    fetchSpy.mockImplementation(async (url: unknown) => {
      fetchCalls.push(`GET ${url}`);
      const urlStr = url as string;
      if (urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providersResponse), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    const providerSelect = wrapper.find('select[aria-label="Provider"]');
    expect(providerSelect.exists()).toBe(true);
    const options = providerSelect.findAll('option');
    expect(options.length).toBe(3); // vide + anthropic + openai

    // Choisir anthropic → select Modèle contient 2 modèles
    await providerSelect.setValue('anthropic');
    await providerSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = wrapper.find('select[aria-label="Modèle"]');
    const modelOptions = modelSelect.findAll('option');
    expect(modelOptions.length).toBe(3);
    expect(modelOptions[1].text()).toBe('claude-sonnet-4-20250514');
    expect(modelOptions[2].text()).toBe('claude-haiku-4-20250514');
    expect((modelSelect.element as HTMLSelectElement).value).toBe('');

    expect(wrapper.text()).not.toContain('Aucun modèle disponible');
  });

  it('POST corps inchangé : { name, provider, model, ... }', async () => {
    let created = false;
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const urlStr = url as string;
      const method = (init as RequestInit)?.method ?? 'GET';

      if (method === 'GET' && urlStr.endsWith('/api/model-profiles')) {
        fetchCalls.push(`GET ${urlStr}`);
        if (created) {
          return new Response(JSON.stringify([{
            name: 'new-profile',
            provider: 'anthropic',
            model: 'claude-sonnet-4-20250514',
            temperature: 0.7,
            max_tokens: null,
          }]), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          });
        }
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providersResponse), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'POST' && urlStr.endsWith('/api/model-profiles')) {
        fetchCalls.push('POST /api/model-profiles');
        const body = JSON.parse((init as RequestInit)?.body as string ?? '{}');
        expect(body).toEqual({
          name: 'new-profile',
          provider: 'anthropic',
          model: 'claude-sonnet-4-20250514',
          temperature: 0.7,
        });
        created = true;
        return new Response(JSON.stringify({
          name: 'new-profile',
          provider: 'anthropic',
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

    const nameInput = wrapper.find('input[aria-label="Nom du profil"]');
    await nameInput.setValue('new-profile');
    await new Promise((r) => setTimeout(r, 10));

    const providerSelect = wrapper.find('select[aria-label="Provider"]');
    await providerSelect.setValue('anthropic');
    await providerSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = wrapper.find('select[aria-label="Modèle"]');
    await modelSelect.setValue('claude-sonnet-4-20250514');
    await modelSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));

    const tempInput = wrapper.find('input[aria-label="Température"]');
    await tempInput.setValue('0.7');
    await new Promise((r) => setTimeout(r, 10));

    await (wrapper.find('.btn-create')).trigger('click');
    await new Promise((r) => setTimeout(r, 100));

    expect(created).toBe(true);
    expect(wrapper.text()).toContain('new-profile');
  });

  it('état vide : provider choisi sans modèles → message affiché', async () => {
    const providersNoModels = [
      {
        name: 'anthropic',
        provider_type: 'openai_compatible',
        endpoint: 'https://api.anthropic.com/v1',
        available_models: ['claude-sonnet-4-20250514'],
        is_default: false,
      },
      {
        name: 'custom-provider',
        provider_type: 'openai_compatible',
        endpoint: 'https://custom.example.com/v1',
        available_models: [],
        is_default: false,
      },
    ];
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providersNoModels), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    const providerSelect = wrapper.find('select[aria-label="Provider"]');
    await providerSelect.setValue('custom-provider');
    await providerSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));

    const modelSelect = wrapper.find('select[aria-label="Modèle"]');
    const modelOptions = modelSelect.findAll('option');
    expect(modelOptions.length).toBe(1);

    expect(wrapper.text()).toContain('Aucun modèle disponible');
    expect(wrapper.text()).toContain('lancez un test sur ce provider');
  });

  it('edit — chargement + PUT inchangé', async () => {
    const originalProfile = {
      name: 'aaa',
      provider: 'anthropic',
      model: 'original-model',
      temperature: 0.3,
      max_tokens: 4096,
    };
    const updatedProfile = {
      name: 'aaa',
      provider: 'anthropic',
      model: 'updated-model',
      temperature: 0.5,
      max_tokens: 8192,
    };
    const providers = providersResponse;

    // Sequence of fetch responses (no closures — each call consumes the next one)
    fetchSpy
      // 1: GET /api/model-profiles → mount → [originalProfile]
      .mockResolvedValueOnce(new Response(JSON.stringify([originalProfile]), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }))
      // 2: GET /api/llm-providers → mount → providers
      .mockResolvedValueOnce(new Response(JSON.stringify(providers), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }))
      // 3: PUT /api/model-profiles/aaa → save → UPDATED (assert body in mock)
      .mockImplementationOnce(async (_url: unknown, init: unknown) => {
        const bodyStr = (init as RequestInit)?.body as string;
        const body = JSON.parse(bodyStr ?? '{}');
        // PUT body contains ALL fields from edit form:
        // editProvider='anthropic', editModel='updated-model' (set in test),
        // editTemperature='0.3' (from startEdit), editMaxTokens='4096' (from startEdit)
        expect(body).toEqual({
          provider: 'anthropic',
          model: 'updated-model',
          temperature: 0.3,
          max_tokens: 4096,
        });
        return new Response(JSON.stringify(updatedProfile), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      })
      // 4: GET /api/model-profiles → fetchProfiles after save → [new data]
      //    We need this to return updated data. We'll use a variable to track via mockResolvedValueOnce chain.
      //    Since we can't know the URL, we just return something safe for the next calls.
      // 5: DELETE is not expected in save flow — only in deleteProfile button click
      //    But if it hits, return 204.
      //    Since the chain is consumed, we need a fallback mock.
      ;

    // After the chain is consumed, set a fallback that handles ALL URLs
    fetchSpy.mockImplementation(async (url: unknown, init: unknown) => {
      const method = (init as RequestInit)?.method ?? 'GET';
      const urlStr = url as string;
      if (method === 'GET' && urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify([updatedProfile]), {
          status: 200, headers: { 'Content-Type': 'application/json' },
        });
      }
      if (method === 'GET' && urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providers), {
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
    await new Promise((r) => setTimeout(r, 50));
    await wrapper.vm.$nextTick();

    const editForm = wrapper.find('.form-card:last-of-type');

    // Provider
    const editProviderSelect = editForm.find('select[aria-label="Provider"]');
    expect((editProviderSelect.element as HTMLSelectElement).value).toBe('anthropic');

    // editAvailableModels peuplé par watcher
    const vm = wrapper.vm as any;
    expect(vm.editAvailableModels.length).toBe(2); // claude-sonnet + claude-haiku

    // Model select options
    const editModelSelect = editForm.find('select[aria-label="Modèle"]');
    const editModelOptions = editModelSelect.findAll('option');
    expect(editModelOptions.length).toBe(3); // vide + 2 modèles

    // Mettre à jour editModel directement
    vm.editModel = 'updated-model';
    await new Promise((r) => setTimeout(r, 50));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 50));

    expect(vm.editModel).toBe('updated-model');

// Sauvegarder
    await (wrapper.find('.btn-success')).trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 200));

    // Le PUT a été appelé (expect dans le mock a vérifié le corps)
    // Le re-fetch après PUT retourne updatedProfile
    expect(wrapper.text()).toContain('updated-model');

    // deleteProfile n'est pas appelé dans ce test — s'il arrive (erreur de test),
    // le fallback mock renvoie 204 pour DELETE
  });

  it('edit — changer le provider met à jour editAvailableModels et reset editModel', async () => {
    const profile = {
      name: 'aaa',
      provider: 'anthropic',
      model: 'original-model',
      temperature: 0.3,
max_tokens: 4096,
    };
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify([profile]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify(providersResponse), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ error: 'not found' }), { status: 404 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    await (wrapper.find('.btn-edit')).trigger('click');
    await new Promise((r) => setTimeout(r, 50));
    await wrapper.vm.$nextTick();

    const editForm = wrapper.find('.form-card:last-of-type');
    const editProviderSelect = editForm.find('select[aria-label="Provider"]');

    expect((editProviderSelect.element as HTMLSelectElement).value).toBe('anthropic');

    // Changer vers openai (sans modèles)
    await editProviderSelect.setValue('openai');
    await editProviderSelect.trigger('change');
    await new Promise((r) => setTimeout(r, 10));

    const vm = wrapper.vm as any;
    expect(vm.editAvailableModels).toEqual([]);
    expect((editProviderSelect.element as HTMLSelectElement).value).toBe('openai');

    const editModelSelect = editForm.find('select[aria-label="Modèle"]');
    expect((editModelSelect.element as HTMLSelectElement).value).toBe('');
    expect(wrapper.text()).toContain('Aucun modèle disponible');
  });

  it('erreur GET /api/llm-providers → message affiché', async () => {
    fetchSpy.mockImplementation(async (url: unknown) => {
      const urlStr = url as string;
      if (urlStr.endsWith('/api/model-profiles')) {
        return new Response(JSON.stringify([]), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (urlStr.endsWith('/api/llm-providers')) {
        return new Response(JSON.stringify({ error: 'Network error' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(ModelProfilesScreen);
    await new Promise((r) => setTimeout(r, 50));

    expect(wrapper.text()).toContain('Network error');
  });
});