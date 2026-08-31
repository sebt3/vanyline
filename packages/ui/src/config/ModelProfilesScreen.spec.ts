import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { ConfigRepo, ModelProfile, Provider } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import ModelProfilesScreen from './ModelProfilesScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function repoWith(opts: {
  profiles?: ModelProfile[];
  providers?: Provider[];
  providersError?: Error;
}) {
  const store = new Map((opts.profiles ?? []).map((p) => [p.name, { ...p }]));
  const repo = {
    list: vi.fn(async (domain: string) => {
      if (domain === 'providers') {
        if (opts.providersError) throw opts.providersError;
        return (opts.providers ?? []).map((p) => ({ ...p }));
      }
      return [...store.values()].map((p) => ({ ...p }));
    }),
    create: vi.fn(async (_d: string, item: ModelProfile) => {
      store.set(item.name, { ...item });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<ModelProfile>) => {
      const next = { ...store.get(name)!, ...patch };
      store.set(name, next);
      return next;
    }),
    remove: vi.fn(async (_d: string, name: string) => {
      store.delete(name);
    }),
  };
  return repo as unknown as ConfigRepo;
}

function mountWith(repo: ConfigRepo) {
  return mount(ModelProfilesScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

const ollama: Provider = { name: 'ollama', type: 'ollama', endpoint: 'http://x', available_models: ['qwen2.5', 'llama3'] };
const noModels: Provider = { name: 'bare', type: 'ollama', endpoint: 'http://y', available_models: [] };

async function openCreate(w: ReturnType<typeof mountWith>) {
  await w.find('.btn-create').trigger('click');
  await flushPromises();
  return document.querySelector('[role="dialog"]')!;
}

function selectValue(root: ParentNode, label: string, value: string) {
  const el = root.querySelector(`select[aria-label="${label}"]`) as HTMLSelectElement;
  el.value = value;
  el.dispatchEvent(new Event('change', { bubbles: true }));
}
function inputValue(root: ParentNode, label: string, value: string) {
  const el = root.querySelector(`[aria-label="${label}"]`) as HTMLInputElement;
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('ModelProfilesScreen', () => {
  it('affiche noms, providers (nom direct), modèles', async () => {
    const w = mountWith(
      repoWith({
        profiles: [
          { name: 'chat', provider: 'ollama', model: 'qwen2.5', temperature: 0.7 },
          { name: 'fast', provider: 'ollama', model: 'llama3' },
        ],
        providers: [ollama],
      }),
    );
    await flushPromises();
    expect(w.text()).toContain('chat');
    expect(w.text()).toContain('fast');
    expect(w.text()).toContain('ollama');
    expect(w.text()).toContain('qwen2.5');
  });

  it('le select Provider est alimenté par list("providers")', async () => {
    const repo = repoWith({ providers: [ollama, noModels] });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    const opts = [...dialog.querySelectorAll('select[aria-label="Provider"] option')].map((o) => o.textContent?.trim());
    expect(opts).toContain('ollama');
    expect(opts).toContain('bare');
    expect(repo.list).toHaveBeenCalledWith('providers');
  });

  it('create envoie { name, provider, model }', async () => {
    const repo = repoWith({ providers: [ollama] });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    inputValue(dialog, 'Nom du profil', 'p1');
    selectValue(dialog, 'Provider', 'ollama');
    await flushPromises();
    selectValue(dialog, 'Modèle', 'qwen2.5');
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('profiles', { name: 'p1', provider: 'ollama', model: 'qwen2.5' });
  });

  it('provider sans modèle → message affiché', async () => {
    const w = mountWith(repoWith({ providers: [noModels] }));
    await flushPromises();
    const dialog = await openCreate(w);
    selectValue(dialog, 'Provider', 'bare');
    await flushPromises();
    expect(dialog.textContent).toContain('Aucun modèle disponible');
  });

  it('édition : pré-remplit puis update(name, patch)', async () => {
    const repo = repoWith({
      profiles: [{ name: 'chat', provider: 'ollama', model: 'qwen2.5', temperature: 0.5 }],
      providers: [ollama],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.textContent).toContain('Modifier : chat');
    inputValue(dialog, 'Température', '0.9');
    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();
    expect(repo.update).toHaveBeenCalledWith(
      'profiles',
      'chat',
      expect.objectContaining({ provider: 'ollama', model: 'qwen2.5', temperature: 0.9 }),
    );
  });

  it('édition : changer le provider met à jour editAvailableModels et reset editModel', async () => {
    const other: Provider = { name: 'other', type: 'ollama', endpoint: 'http://z', available_models: ['m-x'] };
    const repo = repoWith({
      profiles: [{ name: 'chat', provider: 'ollama', model: 'qwen2.5' }],
      providers: [ollama, other],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    selectValue(dialog, 'Provider', 'other');
    await flushPromises();
    const vm = w.vm as unknown as { editModel: string; editAvailableModels: string[] };
    expect(vm.editModel).toBe('');
    expect(vm.editAvailableModels).toEqual(['m-x']);
  });

  it('erreur list("providers") → message affiché', async () => {
    const w = mountWith(repoWith({ providersError: new Error('boom providers') }));
    await flushPromises();
    expect(w.text()).toContain('boom providers');
  });

  it('options avancées : clé/valeur → create inclut options avec valeur parsée JSON', async () => {
    const repo = repoWith({ providers: [ollama] });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    inputValue(dialog, 'Nom du profil', 'p1');
    selectValue(dialog, 'Provider', 'ollama');
    await flushPromises();
    selectValue(dialog, 'Modèle', 'qwen2.5');
    (dialog.querySelector('.option-add') as HTMLElement).click();
    await flushPromises();
    inputValue(dialog, 'Option 1 clé', 'top_p');
    inputValue(dialog, 'Option 1 valeur', '0.9');
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('profiles', {
      name: 'p1',
      provider: 'ollama',
      model: 'qwen2.5',
      options: { top_p: 0.9 },
    });
  });

  it('options avancées : ligne sans clé → create sans options', async () => {
    const repo = repoWith({ providers: [ollama] });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    inputValue(dialog, 'Nom du profil', 'p1');
    selectValue(dialog, 'Provider', 'ollama');
    await flushPromises();
    selectValue(dialog, 'Modèle', 'qwen2.5');
    (dialog.querySelector('.option-add') as HTMLElement).click();
    await flushPromises();
    inputValue(dialog, 'Option 1 valeur', 'orpheline');
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('profiles', { name: 'p1', provider: 'ollama', model: 'qwen2.5' });
  });

  it('édition : options existantes pré-remplissent l’éditeur clé/valeur', async () => {
    const repo = repoWith({
      profiles: [{ name: 'chat', provider: 'ollama', model: 'qwen2.5', options: { top_k: 40 } }],
      providers: [ollama],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect((dialog.querySelector('[aria-label="Option 1 clé"]') as HTMLInputElement).value).toBe('top_k');
    expect((dialog.querySelector('[aria-label="Option 1 valeur"]') as HTMLInputElement).value).toBe('40');
  });
});
