import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './client';
import { httpConfigRepo } from './httpConfigRepo';

function jsonResponse<T>(data: T, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

type Handler = (url: string, init: RequestInit) => Response | undefined;

const { handlers, setupFetch } = vi.hoisted(() => {
  const handlers: Handler[] = [];
  function setupFetch() {
    handlers.length = 0;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input: unknown, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : String(input);
      for (const h of handlers) {
        const res = h(url, init ?? {});
        if (res !== undefined) return res;
      }
      throw new Error(`No handler for ${url}`);
    });
  }
  return { handlers, setupFetch };
});

beforeEach(() => {
  setupFetch();
});

/** `GET`, ignore `?page=…`. */
function get(base: string, url: string, init: RequestInit): boolean {
  return (init.method ?? 'GET') === 'GET' && (url === base || url.startsWith(`${base}?`));
}

describe('httpConfigRepo — mapping des domaines', () => {
  it('chaque domaine UI tape le bon endpoint REST', async () => {
    const pairs: Array<[string, string]> = [
      ['providers', '/api/v1/llm-providers'],
      ['profiles', '/api/v1/model-profiles'],
      ['mcp', '/api/v1/mcp-servers'],
      ['toolsets', '/api/v1/toolsets'],
      ['agents', '/api/v1/agents'],
      ['skills', '/api/v1/skills'],
    ];
    for (const [domain, endpoint] of pairs) {
      handlers.length = 0;
      handlers.push((url) => (url.startsWith(endpoint) ? jsonResponse([]) : undefined));
      // profiles/agents préchargent leur domaine de référence
      handlers.push((url) =>
        url.startsWith('/api/v1/llm-providers') || url.startsWith('/api/v1/model-profiles')
          ? jsonResponse([])
          : undefined,
      );
      await expect(httpConfigRepo().list(domain as never)).resolves.toEqual([]);
    }
  });
});

describe('httpConfigRepo — list / dépagination', () => {
  it('dépagine un PagedResult', async () => {
    handlers.push((url) => {
      if (url === '/api/v1/llm-providers') return jsonResponse([]);
      if (url === '/api/v1/model-profiles')
        return jsonResponse({
          items: [{ id: 1, name: 'p1', provider_id: 0, model: 'm' }],
          page: 1,
          per_page: 1,
          total_items: 2,
          total_pages: 2,
        });
      if (url.startsWith('/api/v1/model-profiles?page=2'))
        return jsonResponse({
          items: [{ id: 2, name: 'p2', provider_id: 0, model: 'm' }],
          page: 2,
          per_page: 1,
          total_items: 2,
          total_pages: 2,
        });
      return undefined;
    });
    const items = await httpConfigRepo().list('profiles');
    expect(items.map((i) => i.name)).toEqual(['p1', 'p2']);
  });
});

describe('httpConfigRepo — providers (type ↔ provider_type)', () => {
  it('list traduit provider_type→type, garde available_models/is_default, retire id/api_key null', async () => {
    handlers.push((url) =>
      get('/api/v1/llm-providers', url, {})
        ? jsonResponse([
            {
              id: 1,
              name: 'ol',
              provider_type: 'ollama',
              endpoint: 'http://localhost:11434',
              api_key: null,
              available_models: ['m1'],
              is_default: true,
            },
          ])
        : undefined,
    );
    const [p] = await httpConfigRepo().list('providers');
    expect(p).toEqual({
      name: 'ol',
      type: 'ollama',
      endpoint: 'http://localhost:11434',
      available_models: ['m1'],
      is_default: true,
    });
  });

  it('create traduit type→provider_type et retire les champs web-augmentés', async () => {
    let body: Record<string, unknown> | null = null;
    handlers.push((url, init) => {
      if (url === '/api/v1/llm-providers' && init.method === 'POST') {
        body = JSON.parse(init.body as string);
        return jsonResponse({ id: 9, name: 'x', provider_type: 'ollama', endpoint: 'http://x' });
      }
      if (get('/api/v1/llm-providers', url, init)) return jsonResponse([]);
      return undefined;
    });
    await httpConfigRepo().create('providers', {
      name: 'x',
      type: 'ollama',
      endpoint: 'http://x',
      available_models: ['nope'],
      is_default: true,
    } as never);
    expect(body).toEqual({ name: 'x', provider_type: 'ollama', endpoint: 'http://x' });
  });

  it('create en 403 (RBAC) propage l’ApiError', async () => {
    handlers.push((url, init) => {
      if (url === '/api/v1/llm-providers' && init.method === 'POST')
        return jsonResponse({ error: 'forbidden' }, 403);
      if (get('/api/v1/llm-providers', url, init)) return jsonResponse([]);
      return undefined;
    });
    await expect(
      httpConfigRepo().create('providers', { name: 'p', type: 'ollama', endpoint: 'http://x' } as never),
    ).rejects.toHaveProperty('status', 403);
  });

  it('setDefaultProvider fait PUT /api/v1/llm-providers/{id}/default', async () => {
    let called = false;
    handlers.push((url, init) => {
      if (url === '/api/v1/llm-providers/1/default' && init.method === 'PUT') {
        called = true;
        return jsonResponse({ id: 1, name: 'ol', provider_type: 'ollama', endpoint: 'http://x', is_default: true });
      }
      if (get('/api/v1/llm-providers', url, init))
        return jsonResponse([{ id: 1, name: 'ol', provider_type: 'ollama', endpoint: 'http://x' }]);
      return undefined;
    });
    await httpConfigRepo().setDefaultProvider('ol');
    expect(called).toBe(true);
  });
});

describe('httpConfigRepo — profiles (provider ↔ provider_id)', () => {
  const providers = [{ id: 7, name: 'ollama-local', provider_type: 'ollama', endpoint: 'http://x' }];

  it('list traduit provider_id→nom, retire id/owner_id, omet les optionnels null/vides', async () => {
    handlers.push((url) => {
      if (get('/api/v1/llm-providers', url, {})) return jsonResponse(providers);
      if (get('/api/v1/model-profiles', url, {}))
        return jsonResponse([
          {
            id: 1,
            owner_id: 1,
            name: 'chat',
            provider_id: 7,
            model: 'qwen',
            temperature: 0.7,
            max_tokens: null,
            options: {},
          },
        ]);
      return undefined;
    });
    const [mp] = await httpConfigRepo().list('profiles');
    expect(mp).toEqual({ name: 'chat', provider: 'ollama-local', model: 'qwen', temperature: 0.7 });
  });

  it('create traduit provider (nom)→provider_id dans le body', async () => {
    let body: Record<string, unknown> | null = null;
    handlers.push((url, init) => {
      if (get('/api/v1/llm-providers', url, init)) return jsonResponse(providers);
      if (url === '/api/v1/model-profiles' && init.method === 'POST') {
        body = JSON.parse(init.body as string);
        return jsonResponse({ id: 5, name: 'x', provider_id: 7, model: 'm' });
      }
      if (get('/api/v1/model-profiles', url, init)) return jsonResponse([]);
      return undefined;
    });
    await httpConfigRepo().create('profiles', { name: 'x', provider: 'ollama-local', model: 'm' } as never);
    expect(body).toEqual({ name: 'x', provider_id: 7, model: 'm' });
  });

  it('create avec provider inconnu → ApiError', async () => {
    handlers.push((url, init) => {
      if (get('/api/v1/llm-providers', url, init)) return jsonResponse(providers);
      if (get('/api/v1/model-profiles', url, init)) return jsonResponse([]);
      return undefined;
    });
    await expect(
      httpConfigRepo().create('profiles', { name: 'x', provider: 'inconnu', model: 'm' } as never),
    ).rejects.toBeInstanceOf(ApiError);
  });
});

describe('httpConfigRepo — agents (model ↔ model_profile_id)', () => {
  const providers = [{ id: 1, name: 'p', provider_type: 'ollama', endpoint: 'http://x' }];
  const profiles = [{ id: 2, owner_id: 1, name: 'qwen', provider_id: 1, model: 'qwen2.5' }];

  it('list traduit model_profile_id→nom du profil', async () => {
    handlers.push((url) => {
      if (get('/api/v1/llm-providers', url, {})) return jsonResponse(providers);
      if (get('/api/v1/model-profiles', url, {})) return jsonResponse(profiles);
      if (get('/api/v1/agents', url, {}))
        return jsonResponse([
          {
            id: 1,
            owner_id: 1,
            name: 'coder',
            mode: 'primary',
            model_profile_id: 2,
            toolsets: ['dev'],
            skills: 'auto',
            system_prompt: 'p',
          },
        ]);
      return undefined;
    });
    const [a] = await httpConfigRepo().list('agents');
    expect(a).toMatchObject({ name: 'coder', model: 'qwen', mode: 'primary', toolsets: ['dev'], skills: 'auto' });
    expect(a).not.toHaveProperty('model_profile_id');
    expect(a).not.toHaveProperty('owner_id');
  });

  it('update ne met dans le body que les clés du patch, model→model_profile_id', async () => {
    let body: Record<string, unknown> | null = null;
    handlers.push((url, init) => {
      if (get('/api/v1/llm-providers', url, init)) return jsonResponse(providers);
      if (get('/api/v1/model-profiles', url, init)) return jsonResponse(profiles);
      if (url === '/api/v1/agents/1' && init.method === 'PUT') {
        body = JSON.parse(init.body as string);
        return jsonResponse({ id: 1, name: 'coder', model_profile_id: 2 });
      }
      if (get('/api/v1/agents', url, init))
        return jsonResponse([{ id: 1, owner_id: 1, name: 'coder', mode: 'primary', model_profile_id: 2 }]);
      return undefined;
    });
    await httpConfigRepo().update('agents', 'coder', { model: 'qwen' } as never);
    expect(body).toEqual({ model_profile_id: 2 });
  });
});

describe('httpConfigRepo — mcp / toolsets / skills', () => {
  it('list mcp traduit server_type→type, garde available_tools', async () => {
    handlers.push((url) =>
      get('/api/v1/mcp-servers', url, {})
        ? jsonResponse([
            { id: 1, name: 'fs', server_type: 'http-streamable', url: 'http://mcp:3000', headers: {}, available_tools: ['read'] },
          ])
        : undefined,
    );
    const [s] = await httpConfigRepo().list('mcp');
    expect(s).toEqual({ name: 'fs', type: 'http-streamable', url: 'http://mcp:3000', available_tools: ['read'] });
  });

  it('update toolsets — patch partiel, seul local_tools dans le body', async () => {
    let body: Record<string, unknown> | null = null;
    handlers.push((url, init) => {
      if (url === '/api/v1/toolsets/1' && init.method === 'PUT') {
        body = JSON.parse(init.body as string);
        return jsonResponse({ id: 1, name: 'dev' });
      }
      if (get('/api/v1/toolsets', url, init))
        return jsonResponse([{ id: 1, owner_id: 1, name: 'dev', local_tools: [], mcp: [] }]);
      return undefined;
    });
    await httpConfigRepo().update('toolsets', 'dev', { local_tools: ['bash', 'git'] } as never);
    expect(body).toEqual({ local_tools: ['bash', 'git'] });
  });

  it('get skills fait GET /{id} et renvoie le body ; list ne renvoie pas le body', async () => {
    handlers.push((url) => {
      if (url === '/api/v1/skills/3')
        return jsonResponse({ id: 3, owner_id: 1, name: 'sk', description: 'd', body: '# corps' });
      if (get('/api/v1/skills', url, {}))
        return jsonResponse([{ id: 3, name: 'sk', description: 'd' }]);
      return undefined;
    });
    const repo = httpConfigRepo();
    const [meta] = await repo.list('skills');
    expect(meta).toEqual({ name: 'sk', description: 'd' });
    const detail = await repo.get('skills', 'sk');
    expect(detail).toEqual({ name: 'sk', description: 'd', body: '# corps' });
  });

  it('get avec un nom inconnu rejette', async () => {
    handlers.push((url) =>
      get('/api/v1/skills', url, {}) ? jsonResponse([{ id: 1, name: 'sk', description: 'd' }]) : undefined,
    );
    await expect(httpConfigRepo().get('skills', 'absent')).rejects.toBeInstanceOf(ApiError);
  });
});

describe('httpConfigRepo — actions', () => {
  it('remove résout name→id puis DELETE', async () => {
    let deleted = false;
    handlers.push((url, init) => {
      if (url === '/api/v1/mcp-servers/1' && init.method === 'DELETE') {
        deleted = true;
        return new Response(null, { status: 204 });
      }
      if (get('/api/v1/mcp-servers', url, init))
        return jsonResponse([{ id: 1, name: 'fs', server_type: 'http-streamable', url: 'http://x' }]);
      return undefined;
    });
    await httpConfigRepo().remove('mcp', 'fs');
    expect(deleted).toBe(true);
  });

  it('testProvider / testMcpServer résolvent name→id puis POST /test', async () => {
    handlers.push((url, init) => {
      if (url === '/api/v1/llm-providers/1/test' && init.method === 'POST')
        return jsonResponse({ models: ['a'] });
      if (url === '/api/v1/mcp-servers/2/test' && init.method === 'POST')
        return jsonResponse({ tools: ['t'] });
      if (get('/api/v1/llm-providers', url, init))
        return jsonResponse([{ id: 1, name: 'ol', provider_type: 'ollama', endpoint: 'http://x' }]);
      if (get('/api/v1/mcp-servers', url, init))
        return jsonResponse([{ id: 2, name: 'fs', server_type: 'http-streamable', url: 'http://x' }]);
      return undefined;
    });
    const repo = httpConfigRepo();
    expect(await repo.testProvider('ol')).toEqual({ models: ['a'] });
    expect(await repo.testMcpServer('fs')).toEqual({ tools: ['t'] });
  });

  it('listLocalTools renvoie les noms', async () => {
    handlers.push((url) =>
      url === '/api/local-tools'
        ? jsonResponse([
            { name: 'bash', description: 'x' },
            { name: 'read', description: 'y' },
          ])
        : undefined,
    );
    expect(await httpConfigRepo().listLocalTools()).toEqual(['bash', 'read']);
  });
});
