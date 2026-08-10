import { describe, expect, it, vi, beforeEach } from 'vitest';
import { ApiError, createApiClient } from './client';

describe('createApiClient', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockClear();
  });

  it("get envoie credentials: 'include' et renvoie le JSON typé", async () => {
    const client = createApiClient();
    const mockData = { name: 'foo' };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    const result = await client.get<{ name: string }>('/api/resource');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/resource',
      expect.objectContaining({
        method: 'GET',
        credentials: 'include',
        headers: {},
      }),
    );
    expect(result).toEqual(mockData);
  });

  it("post avec corps JSON envoie Content-Type et le corps sérialisé", async () => {
    const client = createApiClient();
    const mockData = { created: true };
    const input = { key: 'value' };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    await client.post('/api/resource', input);

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/resource',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(input),
      }),
    );
  });

  it("post sans corps envoie aucun Content-Type et pas de body", async () => {
    const client = createApiClient();
    const mockData = { created: true };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 201 }),
    );

    await client.post<{ created: boolean }>('/api/resource', undefined);

    // body: undefined → la clé n'apparaît pas dans l'objet RequestInit de jsdom
    const callArgs = (fetchSpy as any).mock.calls[0];
    const init = callArgs[1] as RequestInit;
    expect(init.method).toBe('POST');
    expect(init.credentials).toBe('include');
    expect(init.headers).toEqual({});
    expect(init.body).toBeUndefined();
  });

  it("réponse non-ok avec corps JSON lève ApiError avec code extrait", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockResolvedValue(
      new Response(
        JSON.stringify({ error: 'VNL-LLM-002: Model not found' }),
        { status: 422, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    try {
      await client.get('/api/resource');
      expect.unreachable('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      const apiErr = err as ApiError;
      expect(apiErr.status).toBe(422);
      expect(apiErr.code).toBe('VNL-LLM-002');
      expect(apiErr.message).toBe('VNL-LLM-002: Model not found');
    }
  });

  it("réponse non-ok sans corps JSON lève ApiError avec message HTTP <status>", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockResolvedValue(
      new Response(null, { status: 500 }),
    );

    try {
      await client.get('/api/resource');
      expect.unreachable('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      const apiErr = err as ApiError;
      expect(apiErr.status).toBe(500);
      expect(apiErr.code).toBeUndefined();
      expect(apiErr.message).toBe('HTTP 500');
    }
  });

  it("réponse non-ok avec corps non-JSON lève ApiError HTTP <status>", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockResolvedValue(
      new Response('not json', {
        status: 500,
        headers: { 'Content-Type': 'text/plain' },
      }),
    );

    try {
      await client.get('/api/resource');
      expect.unreachable('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      const apiErr = err as ApiError;
      expect(apiErr.status).toBe(500);
      expect(apiErr.message).toBe('HTTP 500');
    }
  });

  it("delete sur 204 ne tente pas de parser et résout void", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockResolvedValue(
      new Response(null, { status: 204 }),
    );

    await expect(client.delete('/api/resource')).resolves.toBeUndefined();
    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/resource',
      expect.objectContaining({
        method: 'DELETE',
        credentials: 'include',
        headers: {},
      }),
    );
  });

  it("erreur réseau lève ApiError avec status: 0", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockRejectedValue(new Error('ENOTFOUND'));

    try {
      await client.get('/api/resource');
      expect.unreachable('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      const apiErr = err as ApiError;
      expect(apiErr.status).toBe(0);
      expect(apiErr.message).toContain('Network error');
    }
  });

  it("delete erreur réseau lève ApiError avec status: 0", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockRejectedValue(new Error('ENOTFOUND'));

    try {
      await client.delete('/api/resource');
      expect.unreachable('should have thrown');
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      const apiErr = err as ApiError;
      expect(apiErr.status).toBe(0);
      expect(apiErr.message).toContain('Network error');
    }
  });

  it("put envoie credentials et body sérialisé", async () => {
    const client = createApiClient();

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify({ updated: true }), { status: 200 }),
    );

    await client.put('/api/resource', { name: 'bar' });

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/resource',
      expect.objectContaining({
        method: 'PUT',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 'bar' }),
      }),
    );
  });

  it("utilise baseUrl pour construire le chemin", async () => {
    const client = createApiClient('http://localhost:8080');

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify({ a: 1 }), { status: 200 }),
    );

    await client.get<{ a: number }>('/test');

    expect(fetchSpy).toHaveBeenCalledWith(
      'http://localhost:8080/test',
      expect.any(Object),
    );
  });
});