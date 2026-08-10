import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import * as clientModule from '../../api/client';
import ModelProfilesScreen from './ModelProfilesScreen.vue';

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('ModelProfilesScreen', () => {
  it('GET /api/model-profiles renvoie 2 → affiche noms, providers, modèles', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: new Map([['content-type', 'application/json']]),
      json: () =>
        Promise.resolve([
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
        ]),
    } as unknown as Response);

    const wrapper = mount(ModelProfilesScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

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

  it('formulaire de création → POST avec corps attendu puis re-fetch', async () => {
    let fetchCount = 0;
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockImplementation(async (_url, init) => {
      const method = (init?.method ?? 'GET') as string;
      if (method === 'GET') {
        fetchCount++;
        if (fetchCount === 1) {
          return {
            ok: true,
            status: 200,
            headers: new Map([['content-type', 'application/json']]),
            json: () => Promise.resolve([]),
          } as unknown as Response;
        }
        return {
          ok: true,
          status: 200,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve([
              {
                name: 'new-profile',
                provider: 'anthropic',
                model: 'claude-sonnet-4-20250514',
                temperature: 0.7,
                max_tokens: null,
              },
            ]),
        } as unknown as Response;
      }
      if (method === 'POST') {
        const body = JSON.parse((init?.body as string) ?? '{}');
        expect(body).toEqual({
          name: 'new-profile',
          provider: 'anthropic',
          model: 'claude-sonnet-4-20250514',
          temperature: 0.7,
        });
        return {
          ok: true,
          status: 201,
          headers: new Map([['content-type', 'application/json']]),
          json: () =>
            Promise.resolve({
              name: 'new-profile',
              provider: 'anthropic',
              model: 'claude-sonnet-4-20250514',
              temperature: 0.7,
              max_tokens: null,
            }),
        } as unknown as Response;
      }
      return new Response(null, { status: 500 });
    });

    const wrapper = mount(ModelProfilesScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const inputs = wrapper.findAll('input[type="text"], input[type="number"]');
    await (inputs[0] as any).setValue('new-profile');
    await (inputs[1] as any).setValue('anthropic');
    await (inputs[2] as any).setValue('claude-sonnet-4-20250514');
    await (inputs[3] as any).setValue('0.7');
    await (inputs[4] as any).setValue('');

    const createBtn = wrapper.find('.btn-create');
    await createBtn.trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('new-profile');
  });

  it('"Modifier" charge les valeurs puis "Sauvegarder" → PUT avec corps puis re-fetch', async () => {
    const mockGet = vi.fn().mockImplementation(async () => {
      // First call: initial data. Subsequent calls (after PUT): updated data
      const callCount = mockGet.mock.calls.length;
      return callCount === 1
        ? [{ name: 'aaa', provider: 'anthropic', model: 'original-model', temperature: 0.3, max_tokens: 4096 }]
        : [{ name: 'aaa', provider: 'anthropic', model: 'updated-model', temperature: 0.5, max_tokens: 8192 }];
    });
    const mockPut = vi.fn().mockResolvedValue({ name: 'aaa', provider: 'anthropic', model: 'updated-model', temperature: 0.5, max_tokens: 8192 });
    const mockDelete = vi.fn().mockResolvedValue(undefined);
    const mockPost = vi.fn().mockResolvedValue({});

    vi.spyOn(clientModule, 'createApiClient').mockReturnValue({
      get: mockGet,
      put: mockPut,
      delete: mockDelete,
      post: mockPost,
    });

    const wrapper = mount(ModelProfilesScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(mockGet).toHaveBeenCalled();
    expect(mockGet.mock.calls[0][0]).toBe('/api/model-profiles');

    // Vérifier le chargement initial
    expect(wrapper.text()).toContain('original-model');

    // Ouvrir l'édition
    await (wrapper.find('.btn-edit') as any).trigger('click');
    await wrapper.vm.$nextTick();

    // Vérifier le formulaire d'édition pré-rempli
    const editCard = wrapper.findAll('.form-card')[1];
    const editInputs = editCard.findAll('input');
    expect(editInputs[1].element.value).toBe('original-model');

    // Mettre à jour les refs réactives
    (wrapper.vm as any).editModel = 'updated-model';
    (wrapper.vm as any).editTemperature = '0.5';
    await wrapper.vm.$nextTick();

    // Sauvegarder
    await (wrapper.find('.btn-success') as any).trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Vérifier que PUT a été appelé avec les bons arguments
    expect(mockPut).toHaveBeenCalledWith('/api/model-profiles/aaa', {
      provider: 'anthropic',
      model: 'updated-model',
      temperature: 0.5,
      max_tokens: 4096,
    });

    // Vérifier le rendu mis à jour
    expect(wrapper.text()).toContain('updated-model');
    expect(wrapper.text()).toContain('0.5');
  });

  it('"Supprimer" → DELETE /api/model-profiles/{name} puis re-fetch', async () => {
    let dataPhase = 0;
    const mockGet = vi.fn().mockImplementation(async () => {
      dataPhase++;
      return dataPhase === 1
        ? [{ name: 'aaa', provider: 'anthropic', model: 'claude-sonnet-4-20250514', temperature: 0.4, max_tokens: 4096 }]
        : [];
    });
    const mockDelete = vi.fn().mockResolvedValue(undefined);
    const mockPut = vi.fn().mockResolvedValue({});
    const mockPost = vi.fn().mockResolvedValue({});

    vi.spyOn(clientModule, 'createApiClient').mockReturnValue({
      get: mockGet,
      delete: mockDelete,
      put: mockPut,
      post: mockPost,
    });

    const wrapper = mount(ModelProfilesScreen);

    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    await (wrapper.find('.btn-delete') as any).trigger('click');
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(wrapper.text()).toContain('Aucun profil');

    expect(mockDelete).toHaveBeenCalledWith('/api/model-profiles/aaa');
  });
});