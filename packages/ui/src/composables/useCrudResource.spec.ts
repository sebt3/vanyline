import { describe, expect, it, vi } from 'vitest';
import type { ConfigRepo } from '../ports';
import { useCrudResource } from './useCrudResource';

function fakeRepo(over: Partial<ConfigRepo> = {}): ConfigRepo {
  return {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    remove: vi.fn().mockResolvedValue(undefined),
    setDefaultProvider: vi.fn(),
    testProvider: vi.fn(),
    testMcpServer: vi.fn(),
    listLocalTools: vi.fn(),
    ...over,
  } as ConfigRepo;
}

describe('useCrudResource (ui)', () => {
  it('fetch alimente items et éteint loading', async () => {
    const repo = fakeRepo({ list: vi.fn().mockResolvedValue([{ name: 'a', description: 'x' }]) });
    const r = useCrudResource(repo, 'skills');
    expect(r.loading.value).toBe(true);
    await r.fetch();
    expect(r.items.value).toEqual([{ name: 'a', description: 'x' }]);
    expect(r.loading.value).toBe(false);
    expect(r.error.value).toBeNull();
  });

  it('fetch capture l’erreur dans error (n’explose pas)', async () => {
    const repo = fakeRepo({ list: vi.fn().mockRejectedValue(new Error('boom')) });
    const r = useCrudResource(repo, 'providers');
    await r.fetch();
    expect(r.error.value).toBe('boom');
  });

  it('create appelle repo.create(domain, item) puis refetch, et propage l’erreur', async () => {
    const create = vi.fn().mockResolvedValue({ name: 'n' });
    const list = vi.fn().mockResolvedValue([]);
    const r = useCrudResource(fakeRepo({ create, list }), 'agents');
    await r.create({ name: 'n' } as never);
    expect(create).toHaveBeenCalledWith('agents', { name: 'n' });
    expect(list).toHaveBeenCalled();

    const rErr = useCrudResource(fakeRepo({ create: vi.fn().mockRejectedValue(new Error('403')) }), 'providers');
    await expect(rErr.create({ name: 'x' } as never)).rejects.toThrow('403');
  });

  it('update appelle repo.update(domain, name, patch) puis refetch', async () => {
    const update = vi.fn().mockResolvedValue({ name: 'n' });
    const list = vi.fn().mockResolvedValue([]);
    const r = useCrudResource(fakeRepo({ update, list }), 'toolsets');
    await r.update('n', { prompt: 'p' } as never);
    expect(update).toHaveBeenCalledWith('toolsets', 'n', { prompt: 'p' });
    expect(list).toHaveBeenCalled();
  });

  it('remove capture l’erreur dans error', async () => {
    const r = useCrudResource(fakeRepo({ remove: vi.fn().mockRejectedValue(new Error('nope')) }), 'mcp');
    await r.remove('x');
    expect(r.error.value).toBe('nope');
  });
});
