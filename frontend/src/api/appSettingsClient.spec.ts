import { afterEach, describe, expect, it, vi } from 'vitest';
import { createApiClient } from './client';
import { getDefaultModel, isAdmin, saveDefaultModel } from './appSettingsClient';

// Scenarios de appSettingsClient.sdd. fetch est remplacé par route (méthode + URL exacte).

type Reply = { status?: number; contentType?: string; body?: unknown; html?: string };

function routeFetch(routes: Record<string, Reply>) {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockClear();
  spy.mockImplementation(async (input, init) => {
    const key = `${(init?.method ?? 'GET').toUpperCase()} ${String(input)}`;
    const r = routes[key] ?? { status: 500, body: { error: `non routé : ${key}` } };
    const status = r.status ?? 200;
    const isHtml = r.contentType === 'text/html' || r.html !== undefined;
    return {
      ok: status < 400,
      status,
      headers: new Map([['content-type', isHtml ? 'text/html' : 'application/json']]),
      json: () =>
        isHtml
          ? Promise.reject(new SyntaxError('Unexpected token <'))
          : Promise.resolve(r.body),
    } as unknown as Response;
  });
  return spy;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('appSettingsClient', () => {
  it('admin_reconnu_par_son_groupe', async () => {
    routeFetch({ 'GET /api/v1/me': { body: { groups: ['admin'] } } });
    await expect(isAdmin(createApiClient())).resolves.toBe(true);
  });

  it('admin_reconnu_par_son_groupe : sans le groupe admin, faux', async () => {
    routeFetch({ 'GET /api/v1/me': { body: { groups: [] } } });
    await expect(isAdmin(createApiClient())).resolves.toBe(false);
  });

  it('erreur_de_me_vaut_non_admin : 404 JSON ou page HTML, jamais d\'exception', async () => {
    routeFetch({ 'GET /api/v1/me': { status: 404, body: { error: 'not found' } } });
    await expect(isAdmin(createApiClient())).resolves.toBe(false);

    routeFetch({ 'GET /api/v1/me': { status: 404, html: '<!doctype html><title>404</title>' } });
    await expect(isAdmin(createApiClient())).resolves.toBe(false);

    routeFetch({ 'GET /api/v1/me': { status: 200, html: '<!doctype html><html></html>' } });
    await expect(isAdmin(createApiClient())).resolves.toBe(false);
  });

  it('premiere_sauvegarde_puis_mise_a_jour : POST sans ligne, PUT /app-settings/7 ensuite', async () => {
    const value = { provider_id: 1, model: 'qwen3' };

    const spy = routeFetch({
      'GET /api/v1/app-settings': { body: [{ id: 3, key: 'autre', value: {} }] },
      'POST /api/v1/app-settings': { body: null },
    });
    const current = await getDefaultModel(createApiClient());
    expect(current).toBeNull();
    await saveDefaultModel(current, value, createApiClient());
    const post = spy.mock.calls.find(([u, i]) => u === '/api/v1/app-settings' && i?.method === 'POST');
    expect(post).toBeDefined();
    expect(JSON.parse(String(post![1]?.body))).toEqual({ key: 'default_model', value });

    const spy2 = routeFetch({
      'GET /api/v1/app-settings': { body: [{ id: 7, key: 'default_model', value }] },
      'PUT /api/v1/app-settings/7': { body: null },
    });
    const existing = await getDefaultModel(createApiClient());
    expect(existing).toEqual({ id: 7, value });
    await saveDefaultModel(existing, { provider_id: 2, model: 'llama' }, createApiClient());
    const put = spy2.mock.calls.find(([u, i]) => u === '/api/v1/app-settings/7' && i?.method === 'PUT');
    expect(put).toBeDefined();
    expect(JSON.parse(String(put![1]?.body))).toEqual({
      key: 'default_model',
      value: { provider_id: 2, model: 'llama' },
    });
    expect(spy2.mock.calls.some(([u, i]) => u === '/api/v1/app-settings' && i?.method === 'POST')).toBe(false);
  });
});
