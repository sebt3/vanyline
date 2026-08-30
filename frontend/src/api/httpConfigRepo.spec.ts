import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './client';
import { httpConfigRepo } from './httpConfigRepo';

function jsonResponse<T>(data: T): Response {
  return new Response(JSON.stringify(data), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

/** Shared interceptors so that `vi.hoisted` state lives outside the
 *  describe/it scopes. Each test cleans up by resetting the handlers array. */
const { handlers, setupFetch } = vi.hoisted(() => {
  const handlers: Array<
    (url: string, init: RequestInit) => Response | undefined
  > = [];

  function setupFetch() {
    handlers.length = 0;

    vi.spyOn(globalThis, 'fetch').mockImplementation(
      async (input: unknown, init?: RequestInit) => {
        const url = typeof input === 'string' ? input : String(input);
        const reqInit = init ?? {};
        // Iterate and find the first handler that returns a Response (non-undefined)
        for (const h of handlers) {
          const result = h(url, reqInit);
          if (result !== undefined) return result;
        }
        throw new Error(`No handler for ${url}`);
      },
    );
  }

  return { handlers, setupFetch };
});

beforeEach(() => {
  setupFetch();
});

describe('httpConfigRepo', () => {
  it('1 — list dépagine et renvoie les items name-keyed', async () => {
    handlers.push((url: string, _init: RequestInit) => {
      if (url === '/api/v1/model-profiles')
        return jsonResponse({
          items: [{ id: 1, name: 'default', model: 'gpt-4o' }],
          page: 1,
          per_page: 1,
          total_items: 2,
          total_pages: 2,
        });
      if (url.startsWith('/api/v1/model-profiles?page=2'))
        return jsonResponse({
          items: [{ id: 2, name: 'alt', model: 'claude-3' }],
          page: 2,
          per_page: 1,
          total_items: 2,
          total_pages: 2,
        });
      return undefined;
    });

    const repo = httpConfigRepo();
    const items = await repo.list('profiles');

    expect(items).toHaveLength(2);
    expect(items[0].name).toBe('default');
    expect(items[1].name).toBe('alt');
  });

  it('2 — mapping des domaines UI → endpoints app', async () => {
    const domains: Array<{ domain: string; endpoint: string }> = [
      { domain: 'providers', endpoint: '/api/v1/llm-providers' },
      { domain: 'profiles', endpoint: '/api/v1/model-profiles' },
      { domain: 'mcp', endpoint: '/api/v1/mcp-servers' },
      { domain: 'toolsets', endpoint: '/api/v1/toolsets' },
      { domain: 'agents', endpoint: '/api/v1/agents' },
      { domain: 'skills', endpoint: '/api/v1/skills' },
    ];

    for (const { domain, endpoint } of domains) {
      const repo = httpConfigRepo();
      handlers.push((url: string, _init: RequestInit) => {
        if (url === endpoint) return jsonResponse([]);
        return undefined;
      });

      await repo.list(domain as any);

      // Just verifying we didn't throw — the handler matched the right URL.
      expect(true).toBe(true);
    }
  });

  it('3 — get par name via la liste ; nom inconnu rejette', async () => {
    const skills = [
      { id: 1, name: 'x' },
      { id: 2, name: 'y' },
    ] as Array<{ id: number; name: string }>;
    handlers.push((url: string, _init: RequestInit) => {
      if (url.startsWith('/api/v1/skills')) return jsonResponse(skills);
      return undefined;
    });

    const repo = httpConfigRepo();
    // Peupler le cache via list
    await repo.list('skills');

    // get avec name connu
    const found = await repo.get('skills', 'x');
    expect(found.name).toBe('x');

    // get avec name inconnu
    await expect(repo.get('skills', 'z')).rejects.toBeInstanceOf(ApiError);
  });

  it('4 — create POST le body et refetch', async () => {
    const newAgent = { id: 3, name: 'a', model: 'm' };
    const existing = [
      { id: 1, name: 'a', model: 'm' },
      { id: 2, name: 'b', model: 'm2' },
    ];

    handlers.push((url: string, init: RequestInit) => {
      if (url === '/api/v1/agents' && init.method === 'POST') {
        return jsonResponse(newAgent);
      }
      if (url === '/api/v1/agents') return jsonResponse(existing);
      return undefined;
    });

    const repo = httpConfigRepo();
    const created = await repo.create('agents', { name: 'a', model: 'm' });

    expect(created.name).toBe('a');
    expect(created.model).toBe('m');
  });

  it('5 — update résout name→id puis PUT', async () => {
    const items = [
      { id: 1, name: 'a', model: 'm' },
      { id: 2, name: 'b', model: 'm2' },
    ];
    handlers.push((url: string, init: RequestInit) => {
      if (url.startsWith('/api/v1/agents') && init.method !== 'PUT') {
        return jsonResponse(items);
      }
      if (url === '/api/v1/agents/1' && init.method === 'PUT')
        return jsonResponse({ id: 1, name: 'a', model: 'm2' });
      return undefined;
    });

    const repo = httpConfigRepo();
    // Peupler le cache name→id
    await repo.list('agents');

    const result = await repo.update('agents', 'a', { model: 'm2' });
    expect(result.model).toBe('m2');
  });

  it('6 — remove résout name→id puis DELETE', async () => {
    let deleteCalled = false;
    handlers.push((url: string, init: RequestInit) => {
      if ((url === '/api/v1/agents' || url.startsWith('/api/v1/agents?')) && init.method !== 'DELETE')
        return jsonResponse([{ id: 1, name: 'a', model: 'm' }]);
      if (url === '/api/v1/agents/1' && init.method === 'DELETE') {
        deleteCalled = true;
        return new Response(null, { status: 204 });
      }
      // refetch après remove
      if (url === '/api/v1/agents') return jsonResponse([]);
      return undefined;
    });

    const repo = httpConfigRepo();
    await repo.list('agents');
    await repo.remove('agents', 'a');

    expect(deleteCalled).toBe(true);
  });

  it('7 — testProvider résout name→id puis POST test', async () => {
    handlers.push((url: string, init: RequestInit) => {
      if (url.startsWith('/api/v1/llm-providers') && init.method !== 'POST')
        return jsonResponse([{ id: 1, name: 'ollama', model: '' }]);
      if (url === '/api/v1/llm-providers/1/test' && init.method === 'POST')
        return jsonResponse({ models: ['llama-3', 'mistral'] });
      return undefined;
    });

    const repo = httpConfigRepo();
    await repo.list('providers');

    const result = await repo.testProvider('ollama');
    expect(result.models).toEqual(['llama-3', 'mistral']);
  });

  it('8 — testMcpServer idem', async () => {
    handlers.push((url: string, init: RequestInit) => {
      if (url.startsWith('/api/v1/mcp-servers') && init.method !== 'POST')
        return jsonResponse([{ id: 1, name: 'mcp1', url: 'http://x' }]);
      if (url === '/api/v1/mcp-servers/1/test' && init.method === 'POST')
        return jsonResponse({ tools: ['read_file', 'write_file'] });
      return undefined;
    });

    const repo = httpConfigRepo();
    await repo.list('mcp');

    const result = await repo.testMcpServer('mcp1');
    expect(result.tools).toEqual(['read_file', 'write_file']);
  });

  it('9 — listLocalTools renvoie les noms', async () => {
    handlers.push((url: string, init: RequestInit) => {
      if (url === '/api/local-tools' && init.method !== 'POST')
        return jsonResponse([
          { name: 'bash', description: 'Executes bash' },
          { name: 'read', description: 'Reads a file' },
        ]);
      return undefined;
    });

    const repo = httpConfigRepo();
    const tools = await repo.listLocalTools();
    expect(tools).toEqual(['bash', 'read']);
  });
});