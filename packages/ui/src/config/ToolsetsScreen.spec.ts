import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { ConfigRepo, McpServer, Toolset } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import ToolsetsScreen from './ToolsetsScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

function repoWith(opts: {
  toolsets?: Toolset[];
  localTools?: string[];
  mcpServers?: McpServer[];
  optionsError?: Error;
}) {
  const store = new Map((opts.toolsets ?? []).map((t) => [t.name, { ...t }]));
  const repo = {
    list: vi.fn(async (domain: string) => {
      if (domain === 'mcp') {
        if (opts.optionsError) throw opts.optionsError;
        return (opts.mcpServers ?? []).map((s) => ({ ...s }));
      }
      return [...store.values()].map((t) => ({ ...t }));
    }),
    listLocalTools: vi.fn(async () => {
      if (opts.optionsError) throw opts.optionsError;
      return opts.localTools ?? [];
    }),
    create: vi.fn(async (_d: string, item: Toolset) => {
      store.set(item.name, { ...item });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<Toolset>) => {
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
  return mount(ToolsetsScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

async function openCreate(w: ReturnType<typeof mountWith>) {
  await w.find('.btn-create').trigger('click');
  await flushPromises();
  return document.querySelector('[role="dialog"]')!;
}

async function check(root: ParentNode, label: string) {
  const box = [...root.querySelectorAll('.checkbox-item')].find((el) => el.textContent?.trim() === label);
  const input = box!.querySelector('input') as HTMLInputElement;
  input.checked = true;
  input.dispatchEvent(new Event('change', { bubbles: true }));
  await flushPromises();
}

describe('ToolsetsScreen', () => {
  it('affiche noms, local_tools, serveurs MCP', async () => {
    const w = mountWith(
      repoWith({
        toolsets: [
          { name: 'dev', description: 'd', local_tools: ['bash'], mcp: [{ server: 'fs', tools: [] }] },
        ],
      }),
    );
    await flushPromises();
    expect(w.text()).toContain('dev');
    expect(w.text()).toContain('bash');
    expect(w.text()).toContain('fs');
  });

  it('create : cocher local tools → create({ name, local_tools, mcp: [] })', async () => {
    const repo = repoWith({ localTools: ['bash', 'git', 'sed'] });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    (dialog.querySelector('input[aria-label="Nom du toolset"]') as HTMLInputElement).value = 'ts1';
    dialog.querySelector('input[aria-label="Nom du toolset"]')!.dispatchEvent(new Event('input', { bubbles: true }));
    await check(dialog, 'bash');
    await check(dialog, 'git');
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('toolsets', {
      name: 'ts1',
      local_tools: ['bash', 'git'],
      mcp: [],
    });
  });

  it('create mcp : ajouter un serveur + cocher tools → create avec mcp:[{server,tools}]', async () => {
    const repo = repoWith({
      localTools: [],
      mcpServers: [{ name: 'fs', type: 'http-streamable', url: 'http://x', available_tools: ['read', 'write'] }],
    });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    (dialog.querySelector('input[aria-label="Nom du toolset"]') as HTMLInputElement).value = 'ts1';
    dialog.querySelector('input[aria-label="Nom du toolset"]')!.dispatchEvent(new Event('input', { bubbles: true }));
    await (dialog.querySelector('.btn-add') as HTMLElement).click();
    await flushPromises();
    const select = dialog.querySelector('select[aria-label="Serveur MCP"]') as HTMLSelectElement;
    select.value = 'fs';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await flushPromises();
    await check(dialog, 'read');
    await check(dialog, 'write');
    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();
    expect(repo.create).toHaveBeenCalledWith('toolsets', {
      name: 'ts1',
      local_tools: [],
      mcp: [{ server: 'fs', tools: ['read', 'write'] }],
    });
  });

  it('serveur MCP sans tools → message, aucune case', async () => {
    const repo = repoWith({
      localTools: [],
      mcpServers: [{ name: 'bare', type: 'http-streamable', url: 'http://x', available_tools: [] }],
    });
    const w = mountWith(repo);
    await flushPromises();
    const dialog = await openCreate(w);
    await (dialog.querySelector('.btn-add') as HTMLElement).click();
    await flushPromises();
    const select = dialog.querySelector('select[aria-label="Serveur MCP"]') as HTMLSelectElement;
    select.value = 'bare';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await flushPromises();
    expect(dialog.textContent).toContain('Aucun outil disponible');
  });

  it('edit : pré-remplit puis update(name, patch)', async () => {
    const repo = repoWith({
      toolsets: [{ name: 'dev', description: 'old', local_tools: ['bash'], mcp: [] }],
      localTools: ['bash', 'git'],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const desc = dialog.querySelector('textarea[aria-label="Description"]') as HTMLTextAreaElement;
    expect(desc.value).toBe('old');
    desc.value = 'new';
    desc.dispatchEvent(new Event('input', { bubbles: true }));
    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();
    expect(repo.update).toHaveBeenCalledWith(
      'toolsets',
      'dev',
      expect.objectContaining({ description: 'new', local_tools: ['bash'] }),
    );
  });

  it('edit : changer le serveur d’une ligne mcp → tools réinitialisés', async () => {
    const repo = repoWith({
      toolsets: [{ name: 'dev', local_tools: [], mcp: [{ server: 'fs', tools: ['read'] }] }],
      mcpServers: [
        { name: 'fs', type: 'http-streamable', url: 'http://x', available_tools: ['read'] },
        { name: 'other', type: 'http-streamable', url: 'http://y', available_tools: ['x'] },
      ],
    });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-edit').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    const select = dialog.querySelector('select[aria-label="Serveur MCP"]') as HTMLSelectElement;
    select.value = 'other';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await flushPromises();
    const vm = w.vm as unknown as { editMcp: Array<{ server: string; tools: string[] }> };
    expect(vm.editMcp[0]).toEqual({ server: 'other', tools: [] });
  });

  it('supprimer : remove(name) → refetch → état vide', async () => {
    const repo = repoWith({ toolsets: [{ name: 'dev', local_tools: [], mcp: [] }] });
    const w = mountWith(repo);
    await flushPromises();
    await w.find('.btn-delete').trigger('click');
    await flushPromises();
    expect(repo.remove).toHaveBeenCalledWith('toolsets', 'dev');
    expect(w.text()).toContain('Aucun toolset');
  });

  it('erreur de chargement des options → message affiché', async () => {
    const w = mountWith(repoWith({ optionsError: new Error('options KO') }));
    await flushPromises();
    expect(w.text()).toContain('options KO');
  });
});
