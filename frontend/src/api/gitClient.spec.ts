import { describe, expect, it, vi, beforeEach } from 'vitest';
import { gitClient } from './gitClient';

describe('gitClient', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockClear();
  });

  it("status appelle GET /api/sandboxes/{name}/git/status", async () => {
    const mockData = { branch: 'main', files: [], clean: true };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    const result = await gitClient.status('sandbox-a');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/sandbox-a/git/status',
      expect.objectContaining({
        method: 'GET',
        credentials: 'include',
        headers: {},
      }),
    );
    expect(result).toEqual(mockData);
  });

  it("diff construit la query avec path encodé et staged", async () => {
    const mockData = { path: 'a b.txt', diff: '@@ -1 +1 @@' };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    // avec staged=true
    await gitClient.diff('s', 'a b.txt', true);
    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/diff?path=a%20b.txt&staged=true',
      expect.any(Object),
    );

    (fetchSpy as any).mockClear();

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    // sans staged
    await gitClient.diff('s', 'x.txt');
    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/diff?path=x.txt',
      expect.any(Object),
    );
  });

  it("stage envoie POST avec le corps { paths }", async () => {
    const mockData = { ok: true };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    await gitClient.stage('s', ['a.txt']);

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/stage',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ paths: ['a.txt'] }),
      }),
    );
  });

  it("commit envoie POST { message }", async () => {
    const mockData = { sha: 'abc123def456', title: 'msg' };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    await gitClient.commit('s', 'msg');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/commit',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: 'msg' }),
      }),
    );
  });

  it("merge retourne le résultat conflicted passthrough", async () => {
    const mockData = { conflicted: true, sha: null };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    const result = await gitClient.merge('s', 'feature-x');

    expect(result).toEqual({ conflicted: true, sha: null });
  });

  it("deleteBranch encode le nom de branche et appelle DELETE", async () => {
    (fetchSpy as any).mockResolvedValue(
      new Response(null, { status: 204 }),
    );

    await gitClient.deleteBranch('s', 'feature/x');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/branches/feature%2Fx',
      expect.objectContaining({
        method: 'DELETE',
        credentials: 'include',
        headers: {},
      }),
    );
  });

  it("log n'ajoute les params que s'ils sont fournis", async () => {
    const mockData = { branch: 'main', commits: [], truncated: false };

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    // sans params
    await gitClient.log('s');
    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/log',
      expect.any(Object),
    );

    (fetchSpy as any).mockClear();

    (fetchSpy as any).mockResolvedValue(
      new Response(JSON.stringify(mockData), { status: 200 }),
    );

    // avec params
    await gitClient.log('s', 50, true);
    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/s/git/log?limit=50&all=true',
      expect.any(Object),
    );
  });
});