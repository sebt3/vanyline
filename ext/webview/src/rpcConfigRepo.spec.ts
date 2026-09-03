import { describe, expect, it, vi } from 'vitest';
import { BridgeRpcError, type BridgeClient } from './bridge';
import { createRpcConfigRepo } from './rpcConfigRepo';
import type { ConfigDomain } from '@vanyline/ui';

// Pont factice typé BridgeClient — seul `request` porte la logique de test
// (les autres méthodes ne sont jamais appelées par le repo). Les assertions
// vérifient `method` et `params` EXACTS (deep equal) appel par appel.
function fakeBridge() {
  const request = vi.fn();
  const bridge: BridgeClient = {
    request,
    chatSend: vi.fn(),
    chatCancel: vi.fn(),
    onChatEvent: vi.fn(),
    onMessage: vi.fn(),
    // Membre requis depuis la tâche 07 ; jamais appelé par le repo (commentaire ci-dessus).
    onConfigChanged: vi.fn(),
  };
  return { bridge, request };
}

describe('rpcConfigRepo — ConfigRepo sur le pont postMessage → RPC', () => {
  it('cas 1 — list : mapping des domaines (profiles→models, mcp→mcpServers, les 4 autres identiques)', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    const cases: Array<[ConfigDomain, string]> = [
      ['providers', 'config/providers'],
      ['profiles', 'config/models'],
      ['mcp', 'config/mcpServers'],
      ['toolsets', 'config/toolsets'],
      ['agents', 'config/agents'],
      ['skills', 'config/skills'],
    ];
    for (const [domain, method] of cases) {
      const items = [{ name: 'x', source: 'workspace' }];
      request.mockResolvedValueOnce(items);
      // Pass-through : le `source` additif est conservé (alimente le badge).
      await expect(repo.list(domain)).resolves.toEqual(items);
      expect(request).toHaveBeenLastCalledWith(method, {});
    }
  });

  it('cas 2 — get skills : config/skills/get, params {name}, résout le détail avec body', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    const detail = { name: 'pdf', description: 'PDF', body: '# corps', source: 'global' };
    request.mockResolvedValueOnce(detail);
    await expect(repo.get('skills', 'pdf')).resolves.toEqual(detail);
    expect(request).toHaveBeenCalledWith('config/skills/get', { name: 'pdf' });
  });

  it('cas 3 — get hors skills : passe par la liste du domaine ; entrée absente → VNL-EXT-023', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request.mockResolvedValueOnce([
      { name: 'orchestrator', mode: 'primary' },
      { name: 'build', mode: 'all' },
    ]);
    await expect(repo.get('agents', 'build')).resolves.toEqual({ name: 'build', mode: 'all' });
    expect(request).toHaveBeenCalledWith('config/agents', {});

    request.mockResolvedValueOnce([{ name: 'orchestrator', mode: 'primary' }]);
    await expect(repo.get('agents', 'absent')).rejects.toThrow(/VNL-EXT-023/);
  });

  it('cas 4 — create : payload sans source ni layer, succès null puis relecture liste', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce([
        { name: 'autre', provider: 'ollama', model: 'm2' },
        { name: 'qwen', provider: 'ollama', model: 'qwen2.5' },
      ]);
    const created = await repo.create('profiles', {
      name: 'qwen',
      provider: 'ollama',
      model: 'qwen2.5',
      source: 'workspace',
    });
    expect(request).toHaveBeenNthCalledWith(1, 'config/models/create', {
      item: { name: 'qwen', provider: 'ollama', model: 'qwen2.5' },
    });
    expect(request).toHaveBeenNthCalledWith(2, 'config/models', {});
    // C'est l'entrée relue (source de vérité serveur) qui est retournée.
    expect(created).toEqual({ name: 'qwen', provider: 'ollama', model: 'qwen2.5' });
  });

  it('cas 5 — create skills : item = {name, description} seuls, body dans l\'enveloppe, relecture via skills/get', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ name: 'pdf', description: 'PDF', body: 'relu', source: 'global' });
    const created = await repo.create('skills', {
      name: 'pdf',
      description: 'PDF',
      body: '# corps',
      source: 'workspace',
    });
    expect(request).toHaveBeenNthCalledWith(1, 'config/skills/create', {
      item: { name: 'pdf', description: 'PDF' },
      body: '# corps',
    });
    expect(request).toHaveBeenNthCalledWith(2, 'config/skills/get', { name: 'pdf' });
    expect(created).toEqual({ name: 'pdf', description: 'PDF', body: 'relu', source: 'global' });
  });

  it('cas 6 — update : params {name, patch} (patch sans source), puis relecture', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce([{ name: 'x', local_tools: [], mcp: [], prompt: 'nouveau' }]);
    const updated = await repo.update('toolsets', 'x', { prompt: 'nouveau', source: 'global' });
    expect(request).toHaveBeenNthCalledWith(1, 'config/toolsets/update', {
      name: 'x',
      patch: { prompt: 'nouveau' },
    });
    expect(request).toHaveBeenNthCalledWith(2, 'config/toolsets', {});
    expect(updated).toEqual({ name: 'x', local_tools: [], mcp: [], prompt: 'nouveau' });
  });

  it('cas 7 — remove : delete {name}, retourne undefined', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request.mockResolvedValueOnce(null);
    await expect(repo.remove('agents', 'x')).resolves.toBeUndefined();
    expect(request).toHaveBeenCalledWith('config/agents/delete', { name: 'x' });
  });

  it('cas 8 — actions test : providers/test → {models}, mcpServers/test → {tools}', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request.mockResolvedValueOnce({ models: ['m1'] });
    await expect(repo.testProvider('ollama')).resolves.toEqual({ models: ['m1'] });
    expect(request).toHaveBeenCalledWith('config/providers/test', { name: 'ollama' });

    request.mockResolvedValueOnce({ tools: ['t1'] });
    await expect(repo.testMcpServer('grafana')).resolves.toEqual({ tools: ['t1'] });
    expect(request).toHaveBeenCalledWith('config/mcpServers/test', { name: 'grafana' });
  });

  it('cas 9 — listLocalTools : descripteurs MCP → noms seuls', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    request.mockResolvedValueOnce([
      { name: 'read_file', description: 'd1', inputSchema: { type: 'object' } },
      { name: 'write_file', description: 'd2', inputSchema: {} },
    ]);
    await expect(repo.listLocalTools()).resolves.toEqual(['read_file', 'write_file']);
    expect(request).toHaveBeenCalledWith('config/localTools', {});
  });

  it('cas 10 — setDefaultProvider : rejet VNL-EXT-024, aucune requête RPC', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    await expect(repo.setDefaultProvider('x')).rejects.toThrow(/VNL-EXT-024/);
    expect(request).not.toHaveBeenCalled();
  });

  it('cas 11 — erreur RPC depuis create : propagation telle quelle (pas de sur-enrobage)', async () => {
    const { bridge, request } = fakeBridge();
    const repo = createRpcConfigRepo(bridge);
    const err = new BridgeRpcError(
      'VNL-RPC-013',
      "VNL-CFG-007: provider 'p' already exists in Workspace layer",
    );
    request.mockRejectedValueOnce(err);
    await expect(
      repo.create('providers', { name: 'p', type: 'ollama', endpoint: 'http://x' }),
    ).rejects.toBe(err);
    // Échec du create → aucune relecture tentée.
    expect(request).toHaveBeenCalledTimes(1);
  });
});
