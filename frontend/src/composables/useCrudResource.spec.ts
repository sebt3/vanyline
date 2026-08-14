import { describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../api/client';
import { ApiError } from '../api/client';
import { useCrudResource } from './useCrudResource';

interface Item {
  id: string;
  name: string;
}

function mockClient(overrides: Partial<ApiClient> = {}): ApiClient {
  return {
    get: vi.fn(),
    post: vi.fn(),
    put: vi.fn(),
    delete: vi.fn(),
    ...overrides,
  };
}

describe('useCrudResource', () => {
  it('fetch() : peuple items et passe loading à false', async () => {
    const items: Item[] = [{ id: '1', name: 'a' }];
    const client = mockClient({ get: vi.fn().mockResolvedValue(items) });
    const resource = useCrudResource<Item>(client, '/api/items');

    expect(resource.loading.value).toBe(true);
    await resource.fetch();

    expect(resource.items.value).toEqual(items);
    expect(resource.loading.value).toBe(false);
    expect(resource.error.value).toBeNull();
    expect(client.get).toHaveBeenCalledWith('/api/items');
  });

  it('fetch() : erreur ApiError → error.value = message, loading false', async () => {
    const client = mockClient({
      get: vi.fn().mockRejectedValue(new ApiError(500, undefined, 'HTTP 500')),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    await resource.fetch();

    expect(resource.error.value).toBe('HTTP 500');
    expect(resource.loading.value).toBe(false);
  });

  it('create() : POST body, refetch, retourne l\'entité créée', async () => {
    const created: Item = { id: '2', name: 'new' };
    const client = mockClient({
      post: vi.fn().mockResolvedValue(created),
      get: vi.fn().mockResolvedValue([created]),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    const result = await resource.create({ name: 'new' });

    expect(client.post).toHaveBeenCalledWith('/api/items', { name: 'new' });
    expect(client.get).toHaveBeenCalledWith('/api/items');
    expect(result).toEqual(created);
    expect(resource.items.value).toEqual([created]);
  });

  it('create() : propage l\'erreur sans toucher error.value (laissé au call site)', async () => {
    const client = mockClient({
      post: vi.fn().mockRejectedValue(new ApiError(400, undefined, 'nom requis')),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    await expect(resource.create({})).rejects.toThrow('nom requis');
    expect(resource.error.value).toBeNull();
  });

  it('update() : PUT sur basePath/id, refetch, retourne l\'entité mise à jour', async () => {
    const updated: Item = { id: '1', name: 'renamed' };
    const client = mockClient({
      put: vi.fn().mockResolvedValue(updated),
      get: vi.fn().mockResolvedValue([updated]),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    const result = await resource.update('1', { name: 'renamed' });

    expect(client.put).toHaveBeenCalledWith('/api/items/1', { name: 'renamed' });
    expect(result).toEqual(updated);
    expect(resource.items.value).toEqual([updated]);
  });

  it('update() : propage l\'erreur sans toucher error.value', async () => {
    const client = mockClient({
      put: vi.fn().mockRejectedValue(new ApiError(404, undefined, 'introuvable')),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    await expect(resource.update('missing', {})).rejects.toThrow('introuvable');
    expect(resource.error.value).toBeNull();
  });

  it('remove() : DELETE sur basePath/id puis refetch', async () => {
    const client = mockClient({
      delete: vi.fn().mockResolvedValue(undefined),
      get: vi.fn().mockResolvedValue([]),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    await resource.remove('1');

    expect(client.delete).toHaveBeenCalledWith('/api/items/1');
    expect(client.get).toHaveBeenCalledWith('/api/items');
    expect(resource.items.value).toEqual([]);
  });

  it('remove() : erreur → error.value = message, ne relance pas', async () => {
    const client = mockClient({
      delete: vi.fn().mockRejectedValue(new ApiError(403, undefined, 'interdit')),
    });
    const resource = useCrudResource<Item>(client, '/api/items');

    await expect(resource.remove('1')).resolves.toBeUndefined();
    expect(resource.error.value).toBe('interdit');
  });
});
