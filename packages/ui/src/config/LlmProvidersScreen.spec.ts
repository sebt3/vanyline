import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { ConfigRepo, Provider } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import LlmProvidersScreen from './LlmProvidersScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function providersRepo(initial: Provider[] = []) {
  const store = new Map(initial.map((p) => [p.name, { ...p }]));
  const repo = {
    list: vi.fn(async () => [...store.values()].map((p) => ({ ...p }))),
    create: vi.fn(async (_d: string, item: Provider) => {
      store.set(item.name, { ...item, available_models: [], is_default: false });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<Provider>) => {
      const prev = store.get(name)!;
      store.delete(name);
      const next = { ...prev, ...patch };
      store.set(next.name, next);
      return next;
    }),
    remove: vi.fn(async (_d: string, name: string) => {
      store.delete(name);
    }),
    setDefaultProvider: vi.fn(async (name: string) => {
      for (const p of store.values()) p.is_default = p.name === name;
    }),
    testProvider: vi.fn(async () => ({ models: ['llama-3', 'mistral'] })),
  };
  return repo as unknown as ConfigRepo;
}

function mountWith(repo: ConfigRepo) {
  return mount(LlmProvidersScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

const ollama: Provider = {
  name: 'ollama-local',
  type: 'ollama',
  endpoint: 'http://localhost:11434',
  is_default: true,
  available_models: [],
};
const openai: Provider = {
  name: 'openai-proxy',
  type: 'openai-compatible',
  endpoint: 'https://openai.example.com/v1',
  api_key: 'sk-test',
  is_default: false,
  available_models: ['gpt-4'],
};

describe('LlmProvidersScreen', () => {
  it('affiche noms, types, endpoints et badge défaut', async () => {
    const w = mountWith(providersRepo([ollama, openai]));
    await flushPromises();
    expect(w.text()).toContain('ollama-local');
    expect(w.text()).toContain('openai-compatible');
    expect(w.text()).toContain('https://openai.example.com/v1');
    expect(w.text()).toContain('Défaut');
  });

  it('création : remplir la modale → create(item) sans champ web-augmenté → modale fermée', async () => {
    const repo = providersRepo();
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-create').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;

    const setInput = (label: string, val: string) => {
      const el = dialog.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
      el.value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    setInput('Nom du fournisseur', 'new-prov');
    setInput('Endpoint', 'http://x:11434');

    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();

    expect(repo.create).toHaveBeenCalledWith('providers', {
      name: 'new-prov',
      type: 'ollama',
      endpoint: 'http://x:11434',
    });
    expect((w.vm as unknown as { createModalOpen: boolean }).createModalOpen).toBe(false);
  });

  it('modifier : modale pré-remplie, save appelle update(nomOrigine, patch)', async () => {
    const repo = providersRepo([openai]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.textContent).toContain('Modifier : openai-proxy');
    const endpoint = dialog.querySelector('input[aria-label="Endpoint"]') as HTMLInputElement;
    expect(endpoint.value).toBe('https://openai.example.com/v1');

    endpoint.value = 'https://new.example.com/v1';
    endpoint.dispatchEvent(new Event('input', { bubbles: true }));
    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();

    expect(repo.update).toHaveBeenCalledWith('providers', 'openai-proxy', {
      name: 'openai-proxy',
      type: 'openai-compatible',
      endpoint: 'https://new.example.com/v1',
      api_key: 'sk-test',
    });
    expect((w.vm as unknown as { editModalOpen: boolean }).editModalOpen).toBe(false);
  });

  it('tester : testProvider(name) et affichage des modèles', async () => {
    const repo = providersRepo([ollama]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-test').trigger('click');
    await flushPromises();

    expect(repo.testProvider).toHaveBeenCalledWith('ollama-local');
    expect(w.text()).toContain('llama-3, mistral');
  });

  it('défaut : setDefaultProvider(name) puis re-fetch', async () => {
    const repo = providersRepo([ollama, openai]);
    const w = mountWith(repo);
    await flushPromises();

    // 2e ligne = openai-proxy, son bouton "Défaut"
    const rows = w.findAll('tbody tr');
    await rows[1].find('.btn-default').trigger('click');
    await flushPromises();

    expect(repo.setDefaultProvider).toHaveBeenCalledWith('openai-proxy');
    expect((repo.list as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(1);
  });

  it('supprimer : remove(name) puis re-fetch → état vide', async () => {
    const repo = providersRepo([ollama]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-delete').trigger('click');
    await flushPromises();

    expect(repo.remove).toHaveBeenCalledWith('providers', 'ollama-local');
    expect(w.text()).toContain('Aucun fournisseur LLM');
  });
});
