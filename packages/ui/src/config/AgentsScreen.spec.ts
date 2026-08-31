import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { Agent, ConfigRepo, ModelProfile } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import AgentsScreen from './AgentsScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function repoWith(opts: {
  agents?: Agent[];
  profiles?: ModelProfile[];
  toolsets?: string[];
  skills?: string[];
  optionsError?: Error;
}) {
  const store = new Map((opts.agents ?? []).map((a) => [a.name, { ...a }]));
  const repo = {
    list: vi.fn(async (domain: string) => {
      if (domain === 'agents') return [...store.values()].map((a) => ({ ...a }));
      if (opts.optionsError) throw opts.optionsError;
      if (domain === 'profiles') return (opts.profiles ?? []).map((p) => ({ ...p }));
      if (domain === 'toolsets') return (opts.toolsets ?? []).map((n) => ({ name: n, local_tools: [], mcp: [] }));
      if (domain === 'skills') return (opts.skills ?? []).map((n) => ({ name: n, description: '' }));
      return [];
    }),
    create: vi.fn(async (_d: string, item: Agent) => {
      store.set(item.name, { ...item });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<Agent>) => {
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
  return mount(AgentsScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

const qwen: ModelProfile = { name: 'qwen', provider: 'ollama', model: 'qwen2.5' };

describe('AgentsScreen', () => {
  it('affiche noms, modes, modèles (nom du profil), skills', async () => {
    const w = mountWith(
      repoWith({
        agents: [
          { name: 'coder', mode: 'primary', model: 'qwen', toolsets: ['dev'], skills: 'auto', system_prompt: 'p' },
          { name: 'rev', mode: 'subagent', model: 'qwen', toolsets: [], skills: ['a', 'b'], system_prompt: 'p' },
        ],
        profiles: [qwen],
      }),
    );
    await flushPromises();
    expect(w.text()).toContain('coder');
    expect(w.text()).toContain('primary');
    expect(w.text()).toContain('qwen');
    expect(w.text()).toContain('a, b');
  });

  it('create : model = nom du profil, toolsets en tableau', async () => {
    const repo = repoWith({ profiles: [qwen], toolsets: ['dev'], skills: [] });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-create').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const name = dialog.querySelector('input[aria-label="Nom de l\'agent"]') as HTMLInputElement;
    name.value = 'a1';
    name.dispatchEvent(new Event('input', { bubbles: true }));
    const modelSel = dialog.querySelector('select[aria-label="Profil de modèle"]') as HTMLSelectElement;
    modelSel.value = 'qwen';
    modelSel.dispatchEvent(new Event('change', { bubbles: true }));
    const dev = [...dialog.querySelectorAll('.checkbox-item')].find((el) => el.textContent?.trim() === 'dev');
    const devInput = dev!.querySelector('input') as HTMLInputElement;
    devInput.checked = true;
    devInput.dispatchEvent(new Event('change', { bubbles: true }));
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('agents', {
      name: 'a1',
      mode: 'primary',
      model: 'qwen',
      toolsets: ['dev'],
      skills: 'auto',
      system_prompt: '',
    });
  });

  it('edit agent à skills tableau → cases pré-remplies, save → update', async () => {
    const repo = repoWith({
      agents: [{ name: 'rev', mode: 'primary', model: 'qwen', toolsets: [], skills: ['s1'], system_prompt: 'p' }],
      profiles: [qwen],
      skills: ['s1', 's2'],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const boxes = [...dialog.querySelectorAll('.checkbox-item')];
    const s1 = boxes.find((el) => el.textContent?.trim() === 's1')!.querySelector('input') as HTMLInputElement;
    expect(s1.checked).toBe(true);
    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();
    expect(repo.update).toHaveBeenCalledWith(
      'agents',
      'rev',
      expect.objectContaining({ skills: ['s1'], model: 'qwen' }),
    );
  });

  it('edit agent à skills "auto" → branche select auto/none', async () => {
    const repo = repoWith({
      agents: [{ name: 'coder', mode: 'primary', model: 'qwen', toolsets: [], skills: 'auto', system_prompt: 'p' }],
      profiles: [qwen],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.querySelector('select[aria-label="Skills"]')).toBeTruthy();
  });

  it('supprimer : remove(name) → refetch → état vide', async () => {
    const repo = repoWith({
      agents: [{ name: 'coder', mode: 'primary', model: 'qwen', toolsets: [], skills: 'auto', system_prompt: 'p' }],
      profiles: [qwen],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-delete').trigger('click');
    await flushPromises();
    expect(repo.remove).toHaveBeenCalledWith('agents', 'coder');
    expect(w.text()).toContain('Aucun agent');
  });

  it('erreur de chargement des options → message affiché', async () => {
    const w = mountWith(repoWith({ optionsError: new Error('options KO') }));
    await flushPromises();
    expect(w.text()).toContain('options KO');
  });
});
