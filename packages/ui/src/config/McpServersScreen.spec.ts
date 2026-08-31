import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { ConfigRepo, McpServer } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import McpServersScreen from './McpServersScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function mcpRepo(initial: McpServer[] = [], over: Record<string, unknown> = {}) {
  const store = new Map(initial.map((s) => [s.name, { ...s }]));
  const repo = {
    list: vi.fn(async () => [...store.values()].map((s) => ({ ...s }))),
    create: vi.fn(async (_d: string, item: McpServer) => {
      store.set(item.name, { ...item });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<McpServer>) => {
      const prev = store.get(name)!;
      store.delete(name);
      const next = { ...prev, ...patch };
      store.set(next.name, next);
      return next;
    }),
    remove: vi.fn(async (_d: string, name: string) => {
      store.delete(name);
    }),
    testMcpServer: vi.fn(async () => ({ tools: ['read_file', 'write_file'] })),
    ...over,
  };
  return repo as unknown as ConfigRepo;
}

function mountWith(repo: ConfigRepo) {
  return mount(McpServersScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

const fs: McpServer = { name: 'fs', type: 'http-streamable', url: 'http://mcp:3000' };
const git: McpServer = { name: 'git', type: 'http-streamable', url: 'http://git:3000', available_tools: ['status'] };

describe('McpServersScreen', () => {
  it('affiche noms, types, urls', async () => {
    const w = mountWith(mcpRepo([fs, git]));
    await flushPromises();
    expect(w.text()).toContain('fs');
    expect(w.text()).toContain('git');
    expect(w.text()).toContain('http-streamable');
    expect(w.text()).toContain('http://mcp:3000');
  });

  it('création : create({ name, type, url }) avec le type choisi → modale fermée', async () => {
    const repo = mcpRepo();
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-create').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const set = (label: string, val: string) => {
      const el = dialog.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
      el.value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    set('Nom du serveur', 'new-mcp');
    set('URL', 'https://x/mcp');
    const type = dialog.querySelector('select[aria-label="Type de serveur"]') as HTMLSelectElement;
    type.value = 'http-streamable';
    type.dispatchEvent(new Event('change', { bubbles: true }));
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('mcp', {
      name: 'new-mcp',
      type: 'http-streamable',
      url: 'https://x/mcp',
    });
    expect((w.vm as unknown as { createModalOpen: boolean }).createModalOpen).toBe(false);
  });

  it('le transport sse reste proposé et par défaut', async () => {
    const repo = mcpRepo();
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-create').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const opts = [...dialog.querySelectorAll('select[aria-label="Type de serveur"] option')].map((o) => o.getAttribute('value'));
    expect(opts).toEqual(['sse', 'http-streamable']);
    (dialog.querySelector('input[aria-label="Nom du serveur"]') as HTMLInputElement).value = 's';
    dialog.querySelector('input[aria-label="Nom du serveur"]')!.dispatchEvent(new Event('input', { bubbles: true }));
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('mcp', expect.objectContaining({ type: 'sse' }));
  });

  it('modifier : modale pré-remplie, save → update(nomOrigine, patch)', async () => {
    const repo = mcpRepo([fs]);
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const url = dialog.querySelector('input[aria-label="URL"]') as HTMLInputElement;
    expect(url.value).toBe('http://mcp:3000');
    url.value = 'http://mcp:4000';
    url.dispatchEvent(new Event('input', { bubbles: true }));
    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();
    expect(repo.update).toHaveBeenCalledWith('mcp', 'fs', {
      name: 'fs',
      type: 'http-streamable',
      url: 'http://mcp:4000',
    });
  });

  it('annuler l’édition → pas d’update', async () => {
    const repo = mcpRepo([fs]);
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    await (dialog.querySelector('.btn-cancel') as HTMLElement).click();
    await flushPromises();
    expect(repo.update).not.toHaveBeenCalled();
    expect((w.vm as unknown as { editModalOpen: boolean }).editModalOpen).toBe(false);
  });

  it('supprimer : remove(name) → refetch → état vide', async () => {
    const repo = mcpRepo([fs]);
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-delete').trigger('click');
    await flushPromises();
    expect(repo.remove).toHaveBeenCalledWith('mcp', 'fs');
    expect(w.text()).toContain('Aucun serveur MCP');
  });

  it('découvrir : testMcpServer(name) → tools affichés', async () => {
    const repo = mcpRepo([fs]);
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-discover').trigger('click');
    await flushPromises();
    expect(repo.testMcpServer).toHaveBeenCalledWith('fs');
    expect(w.text()).toContain('2 tools');
  });

  it('découvrir en échec : erreur par ligne, sans planter', async () => {
    const repo = mcpRepo([fs], { testMcpServer: vi.fn().mockRejectedValue(new Error('SSE non implémenté')) });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-discover').trigger('click');
    await flushPromises();
    expect(w.find('.discover-error').text()).toContain('SSE non implémenté');
  });
});
