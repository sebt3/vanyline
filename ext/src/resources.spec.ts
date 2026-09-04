import { describe, expect, it, vi } from 'vitest';
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
  type ResourceNode,
  type RpcLike,
} from './resources';

/** Faux RpcLike : `request(method)` empile `method` dans `calls` et résout (ou rejette)
 *  selon la réponse/raison renvoyée par `get(method)` — le même esprit « faux objet
 *  injecté » que supervisor.spec.ts, sans aucune dépendance à vscode. */
function fakeRpc(get: (method: string) => unknown): RpcLike & { calls: string[] } {
  const calls: string[] = [];
  const request = vi.fn((method: string): unknown => {
    calls.push(method);
    return get(method);
  });
  return { request, calls } as unknown as RpcLike & { calls: string[] };
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

    // phase lue inconnue ⇒ aucune icône connue, description = string brut
    const inconnue = sandboxNode({ metadata: { name: 's' }, status: { phase: 'À propos' } });
    expect(inconnue.iconId).toBeUndefined();
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
  });

  it('cas 8 : échec RPC → [nœud error] portant le message, ne rejette pas', async () => {
    const withError = (err: unknown): Promise<ResourceNode[]> =>
      createResourcesModel(fakeRpc(() => Promise.reject(err))).getRoots();

    const rpcErr = await withError(new RpcError(-32000, 'boom', 'VNL-RPC-010'));
    expect(rpcErr).toHaveLength(1);
    expect(rpcErr[0].kind).toBe('error');
    expect(rpcErr[0].label).toContain('boom');

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
