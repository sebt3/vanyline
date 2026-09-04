import { describe, expect, it, vi, type Mock } from 'vitest';
import { RpcError } from '@vanyline/protocol';
import {
  createResourcesModel,
  extractNamespace,
  ownerNode,
  projectNode,
  PHASE_ICONS,
  projectsOf,
  sandboxNode,
  sandboxesOf,
  validateResourceName,
  validateRepoUrl,
  validateGitRef,
  runProjectCreate,
  runProjectDelete,
  runSandboxCreate,
  runSandboxDelete,
  type PromptApi,
  type ResourceNode,
  type RpcLike,
} from './resources';

/** Faux RpcLike : `request(method, params?)` empile `method` dans `calls` et résout
 *  (ou rejette) selon la réponse/raison renvoyée par `get(method)` — le même esprit
 *  « faux objet injecté » que supervisor.spec.ts, sans aucune dépendance à vscode.
 *  `request.mock.calls` capte aussi les `params` (2ᵉ elt de chaque tuple) — usage dans
 *  les tests create/delete. */
function fakeRpc(get: (method: string) => unknown):
  RpcLike & { calls: string[]; request: Mock<(method: string, params?: unknown) => unknown> } {
  const calls: string[] = [];
  const request = vi.fn((method: string, params?: unknown): unknown => {
    calls.push(method);
    return get(method);
  });
  return { request, calls } as unknown as
    RpcLike & { calls: string[]; request: Mock<(method: string, params?: unknown) => unknown> };
}

/** Réussite owners/list → deux owners (nom + namespace pour le test namespace). */
const ownersList = (
  ...owners: Array<{ metadata: { name: string; namespace?: string } }>
): unknown => owners;

const projectsList = (
  ...projects: Array<{ metadata: { name: string; namespace?: string }; spec?: { owner?: string; repoUrl?: string } }>
): unknown => projects;

const sandboxesList = (
  ...sandboxes: Array<{ metadata: { name: string; namespace?: string }; spec?: { project?: string; branch?: string } }>
): unknown => sandboxes;

describe('createResourcesModel — racine', () => {
  it('cas 1 : owners/list → 2 nœuds owner (label, id, iconId)', async () => {
    const rpc = fakeRpc(() => ownersList(
      { metadata: { name: 'alice' } },
      { metadata: { name: 'bob' } },
    ));
    const model = createResourcesModel(rpc);

    const nodes = await model.getRoots();

    expect(nodes).toHaveLength(2);
    expect(nodes.map((n) => n.label)).toEqual(['alice', 'bob']);
    expect(nodes.map((n) => n.id)).toEqual(['owner:alice', 'owner:bob']);
    expect(nodes.every((n) => n.kind === 'owner' && n.iconId === 'organization')).toBe(true);
  });

  it('cas 2 : getChildren(owner) → projects/list filtré sur spec.owner (seulement alice)', async () => {
    const rpc = fakeRpc(() => projectsList(
      { metadata: { name: 'repo-a' }, spec: { owner: 'alice', repoUrl: 'https://ex/a.git' } },
      { metadata: { name: 'repo-b' }, spec: { owner: 'bob', repoUrl: 'https://ex/b.git' } },
    ));
    const model = createResourcesModel(rpc);

    const kids = await model.getChildren(ownerNode({ metadata: { name: 'alice' } }));

    expect(kids).toHaveLength(1);
    const [kid] = kids;
    expect(kid.kind).toBe('project');
    expect(kid.id).toBe('project:repo-a');
    expect(kid.description).toBe('https://ex/a.git');
  });

  it('cas 3 : getChildren(project) → sandboxes/list filtré sur spec.project', async () => {
    const rpc = fakeRpc(() => sandboxesList(
      { metadata: { name: 's1' }, spec: { project: 'repo-a', branch: 'main' } },
      { metadata: { name: 's2' }, spec: { project: 'other', branch: 'main' } },
    ));
    const model = createResourcesModel(rpc);

    const kids = await model.getChildren(projectNode({ metadata: { name: 'repo-a' } }));

    expect(kids).toHaveLength(1);
    expect(kids[0].kind).toBe('sandbox');
    expect(kids[0].id).toBe('sandbox:s1');
  });

  it('cas 4 : phases → iconId (PHASE_ICONS / question) et description', () => {
    const run = (phase: string | null | undefined): ResourceNode => sandboxNode({ metadata: { name: 's' }, status: { phase } });

    expect(run('Running').iconId).toBe(PHASE_ICONS.Running);
    expect(run('Provisioning').iconId).toBe(PHASE_ICONS.Provisioning);
    expect(run('Failed').iconId).toBe(PHASE_ICONS.Failed);
    expect(run('Suspended').iconId).toBe(PHASE_ICONS.Suspended);
    expect(run('Running').description).toBe('Running');

    expect(run(null).iconId).toBe('question');
    expect(run(null).description).toBe('inconnue');
    expect(run(undefined).iconId).toBe('question');
    expect(run(undefined).description).toBe('inconnue');

    // phase lue inconnue (hors des 4 du controller) ⇒ icône « question », description = string brut
    const inconnue = sandboxNode({ metadata: { name: 's' }, status: { phase: 'À propos' } });
    expect(inconnue.iconId).toBe('question');
    expect(inconnue.description).toBe('À propos');
  });

  it('cas 5 : namespace résolu (undefined, puis owners, puis projects)', async () => {
    expect(createResourcesModel(fakeRpc(() => [])).namespace()).toBeUndefined();
    await createResourcesModel(fakeRpc(() => [])).getRoots();
    expect(createResourcesModel(fakeRpc(() => [])).namespace()).toBeUndefined();

    const owners = createResourcesModel(fakeRpc(() => ownersList({ metadata: { name: 'alice', namespace: 'dev' } })));
    expect(owners.namespace()).toBeUndefined();
    await owners.getRoots();
    expect(owners.namespace()).toBe('dev');

    const projects = createResourcesModel(fakeRpc(() => projectsList({ metadata: { name: 'repo-a', namespace: 'prod' }, spec: { owner: 'alice' } })));
    await projects.getChildren(ownerNode({ metadata: { name: 'alice' } }));
    expect(projects.namespace()).toBe('prod');
  });

  it('cas 6 : pas de cache — deux getRoots → deux owners/list', async () => {
    const rpc = fakeRpc(() => ownersList({ metadata: { name: 'a' } }));
    const model = createResourcesModel(rpc);

    await model.getRoots();
    await model.getRoots();

    expect(rpc.calls.filter((c) => c === 'owners/list')).toHaveLength(2);
  });

  it('cas 7 : serveur absent → [nœud info VNL-EXT-021]', async () => {
    const model = createResourcesModel(undefined);

    const nodes = await model.getRoots();

    expect(nodes).toHaveLength(1);
    expect(nodes[0].kind).toBe('info');
    expect(nodes[0].id).toBe('info:VNL-EXT-021');
    expect(nodes[0].label).toContain('VNL-EXT-021');
    expect(nodes[0].iconId).toBe('info');
  });

  it('cas 8 : échec RPC → [nœud error] portant le message, ne rejette pas', async () => {
    const withError = (err: unknown): Promise<ResourceNode[]> =>
      createResourcesModel(fakeRpc(() => Promise.reject(err))).getRoots();

    const rpcErr = await withError(new RpcError(-32000, 'boom', 'VNL-RPC-010'));
    expect(rpcErr).toHaveLength(1);
    expect(rpcErr[0].kind).toBe('error');
    expect(rpcErr[0].label).toContain('boom');
    expect(rpcErr[0].iconId).toBe('error');

    const plainErr = await withError(new Error('simple'));
    expect(plainErr[0].label).toContain('simple');

    // journalisé via le callback onError optionnel
    const onError = vi.fn();
    const rpc = fakeRpc(() => Promise.reject(new Error('nopool')));
    const model = createResourcesModel(rpc, onError);
    await model.getRoots();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]?.[0]).toContain('owners/list');
  });

  it('cas 9 : getChildren(sandbox) ⇒ []', async () => {
    const model = createResourcesModel(fakeRpc(() => []));
    const kids = await model.getChildren(sandboxNode({ metadata: { name: 's' }, status: { phase: 'Running' } }));
    expect(kids).toEqual([]);
  });
});

describe('helpers purs exportés', () => {
  it('projectsOf : spec.owner === ownerName', () => {
    const list = [
      { metadata: { name: 'a' }, spec: { owner: 'alice' } },
      { metadata: { name: 'b' }, spec: { owner: 'bob' } },
      { metadata: { name: 'c' } },
    ];
    expect(projectsOf(list, 'alice')).toHaveLength(1);
    expect(projectsOf(list, 'alice')[0].metadata.name).toBe('a');
  });

  it('sandboxesOf : spec.project === projectName', () => {
    const list = [
      { metadata: { name: 's1' }, spec: { project: 'repo-a' } },
      { metadata: { name: 's2' }, spec: { project: 'repo-b' } },
    ];
    expect(sandboxesOf(list, 'repo-b').map((s) => s.metadata.name)).toEqual(['s2']);
  });

  it('extractNamespace : premier namespace rencontré, undefined si absent', () => {
    expect(extractNamespace([{ metadata: { namespace: 'dev' } }, { metadata: { namespace: 'prod' } }])).toBe('dev');
    expect(extractNamespace([{ metadata: {} }])).toBeUndefined();
    expect(extractNamespace([])).toBeUndefined();
  });
});

describe('validation de surface (tache 02)', () => {
  it('validateResourceName : valides', () => {
    expect(validateResourceName('repo-a')).toBeUndefined();
    expect(validateResourceName('s1')).toBeUndefined();
    expect(validateResourceName('a')).toBeUndefined();
  });

  it('validateResourceName : invalides ⇒ VNL-EXT-026', () => {
    for (const n of ['', 'Alice', 'mon_projet', '-ab', 'ab-', 'a b', 'a'.repeat(64)]) {
      expect(validateResourceName(n)).toMatch(/VNL-EXT-026/);
    }
  });

  it('validateRepoUrl : valides https + scp', () => {
    expect(validateRepoUrl('https://github.com/a/b.git')).toBeUndefined();
    expect(validateRepoUrl('git@github.com:a/b.git')).toBeUndefined();
  });

  it('validateRepoUrl : invalides ⇒ VNL-EXT-026', () => {
    for (const u of ['', 'pas une url', 'https://']) {
      expect(validateRepoUrl(u)).toMatch(/VNL-EXT-026/);
    }
  });

  it('validateGitRef : valides', () => {
    expect(validateGitRef('main')).toBeUndefined();
    expect(validateGitRef('fix/corrige-1')).toBeUndefined();
  });

  it('validateGitRef : invalides ⇒ VNL-EXT-026', () => {
    for (const r of ['', 'ma branche', 'a..b', '-x']) {
      expect(validateGitRef(r)).toMatch(/VNL-EXT-026/);
    }
  });
});

describe('commandes create/delete (tache 02)', () => {
  /** Faux PromptApi : `prompts` consommées dans l'ordre (par input ET pick),
   *  `confirms` par appel confirm. `confirm` renvoie toujours la prochaine valeur. */
  function scriptedPromptApi(prompts: Array<unknown>, confirms: boolean[] = []): PromptApi {
    let i = 0;
    let c = 0;
    const input = vi.fn(async (): Promise<unknown> => prompts[i++]);
    const pick = vi.fn(async (): Promise<unknown> => prompts[i++]);
    const confirm = vi.fn(async (): Promise<boolean> => confirms[c++]);
    return { input, pick, confirm } as unknown as PromptApi;
  }

  /** Méthodes appelées sur le faux Rpc (tri des tuples method/params de mock.calls). */
  function methodsOf(rpc: ReturnType<typeof fakeRpc>): string[] {
    return rpc.request.mock.calls.map(([m]) => m);
  }

  /** Params du 1ᵉᵉ appel vers une méthode donné sur le faux Rpc. */
  function paramsOf(
    rpc: ReturnType<typeof fakeRpc>,
    method: string,
  ): unknown | undefined {
    return rpc.request.mock.calls.find(([m]) => m === method)?.[1];
  }

  it('runProjectCreate : ownerHint, un seul RPC projects/create, params exacts', async () => {
    const rpc = fakeRpc(() => []);
    const ui = scriptedPromptApi(['repo-a', 'https://ex/a.git', 'dev']);
    const hint = ownerNode({ metadata: { name: 'alice' } });

    const r = await runProjectCreate(rpc, ui, hint);

    expect(r.ok).toBe(true);
    expect(paramsOf(rpc, 'projects/create')).toEqual({
      name: 'repo-a',
      owner: 'alice',
      repoUrl: 'https://ex/a.git',
      defaultBranch: 'dev',
    });
    // hint = nœud owner : aucun owners/list
    expect(methodsOf(rpc)).toEqual(['projects/create']);
  });

  it('runProjectCreate : sans hint, owner pick, branche empty ⇒ params sans defaultBranch', async () => {
    const rpc = fakeRpc((m) => (m === 'owners/list' ? [{ metadata: { name: 'bob' } }] : []));
    const ui = scriptedPromptApi([
      { label: 'bob' },
      'repo-x',
      'https://ex/x.git',
      '',
    ]);

    const r = await runProjectCreate(rpc, ui, undefined);

    expect(r.ok).toBe(true);
    expect(paramsOf(rpc, 'projects/create')).toEqual({
      name: 'repo-x',
      owner: 'bob',
      repoUrl: 'https://ex/x.git',
    });
    expect(methodsOf(rpc)).toEqual(['owners/list', 'projects/create']);
  });

  it('runProjectCreate : annulation au 2e input ⇒ cancelled, aucun write RPC', async () => {
    const rpc = fakeRpc((m) => (m === 'owners/list' ? [{ metadata: { name: 'bob' } }] : []));
    // pick(owner), name, url(=undefined ⇒ annulation)
    const ui = scriptedPromptApi(['x', 'repo-a', undefined]);

    const r = await runProjectCreate(rpc, ui, undefined);

    expect(r).toEqual({ ok: false, cancelled: true, message: '' });
    expect(methodsOf(rpc)).not.toContain('projects/create');
  });

  it('runProjectCreate : re-validation du flux (nom invalide BAD) ⇒ VNL-EXT-026, aucun RPC écrit', async () => {
    const rpc = fakeRpc(() => []);
    const ui = scriptedPromptApi(['BAD', 'https://ex/a.git']);

    const r = await runProjectCreate(rpc, ui, ownerNode({ metadata: { name: 'alice' } }));

    expect(r.ok).toBe(false);
    expect(r.message).toMatch(/VNL-EXT-026/);
    expect(methodsOf(rpc)).not.toContain('projects/create');
  });

  it('runProjectDelete : confirm=false ⇒ cancelled ; confirm=true (hint project) ⇒ delete', async () => {
    // confirm=false ⇒ cancelled, aucun delete
    const rpc1 = fakeRpc(() => []);
    const ui1 = scriptedPromptApi([], [false]);
    const r1 = await runProjectDelete(rpc1, ui1, projectNode({ metadata: { name: 'repo-a' } }));
    expect(r1).toEqual({ ok: false, cancelled: true, message: '' });
    expect(methodsOf(rpc1)).not.toContain('projects/delete');

    // confirm=true, hint project
    const rpc2 = fakeRpc(() => []);
    const ui2 = scriptedPromptApi([], [true]);
    const r2 = await runProjectDelete(rpc2, ui2, projectNode({ metadata: { name: 'repo-b' } }));
    expect(r2.ok).toBe(true);
    expect(r2.message).toBe('Projet repo-b supprimé');
    expect(paramsOf(rpc2, 'projects/delete')).toEqual({ name: 'repo-b' });
  });

  it('runSandboxCreate : projectHint, params exacts', async () => {
    const rpc = fakeRpc(() => []);
    const ui = scriptedPromptApi(['s1', 'main']);
    const hint = projectNode({ metadata: { name: 'repo-a' } });

    const r = await runSandboxCreate(rpc, ui, hint);

    expect(r.ok).toBe(true);
    expect(paramsOf(rpc, 'sandboxes/create')).toEqual({
      name: 's1',
      project: 'repo-a',
      branch: 'main',
    });
  });

  it('runSandboxCreate : sans hint - projects/list d’abord puis pick', async () => {
    const rpc = fakeRpc((m) => {
      if (m === 'projects/list') {
        return [{ metadata: { name: 'repo-a' }, spec: { owner: 'a' } }];
      }
      return [];
    });
    const ui = scriptedPromptApi([{ label: 'repo-a' }, 's1', 'main']);

    const r = await runSandboxCreate(rpc, ui, undefined);

    expect(r.ok).toBe(true);
    expect(methodsOf(rpc)).toEqual(['projects/list', 'sandboxes/create']);
  });

  it('runSandboxDelete : confirm=false ⇒ cancelled ; confirm=true (hint sandbox) ⇒ delete', async () => {
    const rpc1 = fakeRpc(() => []);
    const ui1 = scriptedPromptApi([], [false]);
    const r1 = await runSandboxDelete(
      rpc1,
      ui1,
      sandboxNode({ metadata: { name: 's1' }, status: { phase: 'Running' } }),
    );
    expect(r1).toEqual({ ok: false, cancelled: true, message: '' });
    expect(methodsOf(rpc1)).not.toContain('sandboxes/delete');

    const rpc2 = fakeRpc(() => []);
    const ui2 = scriptedPromptApi([], [true]);
    const r2 = await runSandboxDelete(
      rpc2,
      ui2,
      sandboxNode({ metadata: { name: 's2' }, status: { phase: 'Running' } }),
    );
    expect(r2.ok).toBe(true);
    expect(r2.message).toBe('Sandbox s2 supprimée');
    expect(paramsOf(rpc2, 'sandboxes/delete')).toEqual({ name: 's2' });
  });

  it('rpc absent ⇒ 4 run* VNL-EXT-021, sans ui ni RPC', async () => {
    const ui = scriptedPromptApi(['a', 'b', 'c'], [true]);
    const results = [
      await runProjectCreate(undefined, ui),
      await runProjectDelete(undefined, ui),
      await runSandboxCreate(undefined, ui),
      await runSandboxDelete(undefined, ui),
    ];

    for (const r of results) {
      expect(r.ok).toBe(false);
      expect(r.message).toMatch(/VNL-EXT-021/);
    }
    expect(ui.input).not.toHaveBeenCalled();
    expect(ui.pick).not.toHaveBeenCalled();
    expect(ui.confirm).not.toHaveBeenCalled();
  });

  it('aucun owner : owners/list empty, runProjectCreate sans hint ⇒ VNL-EXT-027', async () => {
    const rpc = fakeRpc((m) => (m === 'owners/list' ? [] : []));
    const ui = scriptedPromptApi([]);

    const r = await runProjectCreate(rpc, ui, undefined);

    expect(r.ok).toBe(false);
    expect(r.message).toMatch(/VNL-EXT-027/);
    expect(methodsOf(rpc)).not.toContain('projects/create');
  });

  it('échec RPC : projects/delete rejette RpcError VNL-RPC-010 ⇒ VNL-EXT-025 + boom, sans rejet', async () => {
    const rpc = fakeRpc((m) =>
      m === 'projects/delete'
        ? Promise.reject(new RpcError(-32000, 'boom', 'VNL-RPC-010'))
        : [],
    );
    const ui = scriptedPromptApi([], [true]);

    const r = await runProjectDelete(rpc, ui, projectNode({ metadata: { name: 'repo-a' } }));

    expect(r.ok).toBe(false);
    expect(r.message).toContain('boom');
    expect(r.message).toContain('VNL-EXT-025');
  });

  it('owners/list rejette (runProjectCreate sans hint) ⇒ VNL-EXT-025, sans rejet hors du run', async () => {
    const rpc = fakeRpc((m) =>
      m === 'owners/list' ? Promise.reject(new RpcError(-32000, 'k8s down', 'VNL-RPC-010')) : [],
    );
    const ui = scriptedPromptApi([]);

    const r = await runProjectCreate(rpc, ui, undefined);

    expect(r.ok).toBe(false);
    expect(r.cancelled).toBeUndefined();
    expect(r.message).toContain('VNL-EXT-025');
    expect(r.message).toContain('owners/list');
    expect(r.message).toContain('k8s down');
    // pas de pick, pas de write RPC : l'échec de liste coupe court
    expect(ui.pick).not.toHaveBeenCalled();
    expect(methodsOf(rpc)).not.toContain('projects/create');
  });

  it('projects/list rejette (runProjectDelete sans hint) ⇒ VNL-EXT-025, sans confirm', async () => {
    const rpc = fakeRpc((m) =>
      m === 'projects/list' ? Promise.reject(new RpcError(-32000, 'boom', 'VNL-RPC-010')) : [],
    );
    const ui = scriptedPromptApi([]);

    const r = await runProjectDelete(rpc, ui, undefined);

    expect(r.ok).toBe(false);
    expect(r.message).toContain('VNL-EXT-025');
    expect(r.message).toContain('boom');
    expect(ui.confirm).not.toHaveBeenCalled();
  });

  it('runSandboxDelete sans hint, sandboxes/list vide ⇒ message NO_SANDBOX (aucune sandbox)', async () => {
    const rpc = fakeRpc((m) => (m === 'sandboxes/list' ? [] : []));
    const ui = scriptedPromptApi([]);

    const r = await runSandboxDelete(rpc, ui, undefined);

    expect(r.ok).toBe(false);
    expect(r.message).toMatch(/VNL-EXT-027/);
    // message spécifique « aucune sandbox », pas le message « aucun Project »
    expect(r.message).toMatch(/aucune Sandbox/i);
    expect(r.message).not.toMatch(/Project/);
  });
});
