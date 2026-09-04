import { mapRpcError } from './panels/bridge';

/** Sérialisation K8s minimale d'un Owner (cf. docs/rpc-protocol.md « Ressources K8s »). */
export interface VnlOwner {
  metadata: { name: string; namespace?: string };
  status?: { pvcName?: string | null; serviceAccount?: string | null };
}

export interface VnlProject {
  metadata: { name: string; namespace?: string };
  spec?: { owner?: string; repoUrl?: string };
}

export interface VnlSandbox {
  metadata: { name: string; namespace?: string };
  spec?: { project?: string; branch?: string };
  status?: { phase?: string | null };
}

/** Sous-ensemble de RpcConnection (@vanyline/protocol) consommé par le modèle.
 *  `conn` du ServerHandle le satisfait structurellement (params optionnel). */
export interface RpcLike {
  request<T>(method: string, params?: unknown): Promise<T>;
}

export type NodeKind = 'owner' | 'project' | 'sandbox' | 'error' | 'info';

export interface ResourceNode {
  readonly kind: NodeKind;
  /** Identité stable entre refresh (TreeItem.id). */
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  /** Id ThemeIcon SANS « … » (ex. 'rocket'). Absent ⇒ pas d'icône. */
  readonly iconId?: string;
  readonly owner?: VnlOwner;
  readonly project?: VnlProject;
  readonly sandbox?: VnlSandbox;
}

/** Namespace resolu = metadata.namespace du premier objet rencontré par une liste
 *  réussie (owners/list d'abord, puis projects/list, sandboxes/list). undefined
 *  tant qu'aucun objet n'a été listé depuis la création du modèle. */
export interface ResourcesModel {
  namespace(): string | undefined;
  /** owners/list. rpc absent ⇒ [nœud info « serveur non démarré »]. */
  getRoots(): Promise<ResourceNode[]>;
  /** owner ⇒ projects/list filtré ; project ⇒ sandboxes/list filtré ;
 *  sandbox/error/info ⇒ []. */
  getChildren(node: ResourceNode): Promise<ResourceNode[]>;
}

/** AUCUN cache : chaque appel getRoots/getChildren fait l'appel RPC (la v1 est au
 *  refresh manuel ; vscode rappelle getChildren sur refresh()). */
export function createResourcesModel(
  rpc: RpcLike | undefined,
  onError?: (line: string) => void,
): ResourcesModel {
  let resolvedNamespace: string | undefined;

  const call = async <T = unknown>(method: string): Promise<T> => {
    if (rpc === undefined) {
      throw new Error('VNL-EXT-021: serveur vanyline non démarré');
    }
    return rpc.request<T>(method);
  };

  /** Met à jour le namespace résolu sur une liste réussie (le premier objet qui en
   *  porte un l'emporte : owners/list d'abord, puis projects, puis sandboxes). */
  const updateNamespace = (
    objs: Array<{ metadata: { namespace?: string } }>,
  ): void => {
    const ns = extractNamespace(objs);
    if (ns !== undefined) {
      resolvedNamespace = ns;
    }
  };

  const onErrorWrap = (method: string, node: ResourceNode): void => {
    onError?.(`VNL-EXT-025: échec de ${method} (${node.label})`);
  };

  return {
    namespace(): string | undefined {
      return resolvedNamespace;
    },

    async getRoots(): Promise<ResourceNode[]> {
      if (rpc === undefined) {
        return [serverNotStartedNode()];
      }
      try {
        const owners = await call<VnlOwner[]>('owners/list');
        const list = Array.isArray(owners) ? owners : [];
        updateNamespace(list);
        return list.map(ownerNode);
      } catch (err) {
        const node = errorNode(err);
        onErrorWrap('owners/list', node);
        return [node];
      }
    },

    async getChildren(node: ResourceNode): Promise<ResourceNode[]> {
      if (node.kind === 'owner') {
        try {
          const projects = await call<VnlProject[]>('projects/list');
          const list = Array.isArray(projects) ? projects : [];
          updateNamespace(list);
          return projectsOf(list, childName('owner:', node)).map(projectNode);
        } catch (err) {
          const n = errorNode(err);
          onErrorWrap('projects/list', n);
          return [n];
        }
      }
      if (node.kind === 'project') {
        try {
          const sandboxes = await call<VnlSandbox[]>('sandboxes/list');
          const list = Array.isArray(sandboxes) ? sandboxes : [];
          updateNamespace(list);
          return sandboxesOf(list, childName('project:', node)).map(sandboxNode);
        } catch (err) {
          const n = errorNode(err);
          onErrorWrap('sandboxes/list', n);
          return [n];
        }
      }
      // sandbox / error / info : feuille, aucune sous-arbre.
      return [];
    },
  };
}

/** Extrait le metadata.namespace du premier objet listé (ordre : owners, projects,
 *  sandboxes). undefined si la liste est vide ou si aucun objet n'en porte un. */
export function extractNamespace(
  objs: Array<{ metadata: { namespace?: string } }>,
): string | undefined {
  for (const obj of objs) {
    const ns = obj.metadata.namespace;
    if (typeof ns === 'string') {
      return ns;
    }
  }
  return undefined;
}

/** Filtre les projects d'un owner (spec.owner === ownerName). */
export function projectsOf(projects: VnlProject[], ownerName: string): VnlProject[] {
  return projects.filter((p) => p.spec?.owner === ownerName);
}

/** Filtre les sandboxes d'un project (spec.project === projectName). */
export function sandboxesOf(sandboxes: VnlSandbox[], projectName: string): VnlSandbox[] {
  return sandboxes.filter((s) => s.spec?.project === projectName);
}

const OWNER_PREFIX = 'owner:';
const PROJECT_PREFIX = 'project:';
const SANDBOX_PREFIX = 'sandbox:';

/** Identité lisible depuis l'id (owner:<name>, project:<name>, …). */
function childName(prefix: string, node: ResourceNode): string {
  return node.id.slice(prefix.length);
}

export function ownerNode(o: VnlOwner): ResourceNode {
  return {
    kind: 'owner',
    id: `${OWNER_PREFIX}${o.metadata.name}`,
    label: o.metadata.name,
    iconId: 'organization',
    owner: o,
  };
}

export function projectNode(p: VnlProject): ResourceNode {
  return {
    kind: 'project',
    id: `${PROJECT_PREFIX}${p.metadata.name}`,
    label: p.metadata.name,
    description: p.spec?.repoUrl,
    iconId: 'repo',
    project: p,
  };
}

/** Phases écrites par le controller (controller/src/sandbox.rs) :
 *  Provisioning | Running | Failed | Suspended. Inconnue/absente ⇒ question. */
export const PHASE_ICONS: Readonly<Record<string, string>> = {
  Running: 'rocket',
  Provisioning: 'sync~spin',
  Failed: 'error',
  Suspended: 'circle-slash',
};

export function sandboxNode(s: VnlSandbox): ResourceNode {
  const phase = s.status?.phase;
  const description = typeof phase === 'string' ? phase : 'inconnue';
  // Phase inconnue (hors des 4 du controller), null ou absente ⇒ même icône « question ».
  const iconId = (typeof phase === 'string' ? PHASE_ICONS[phase] : undefined) ?? 'question';
  return {
    kind: 'sandbox',
    id: `${SANDBOX_PREFIX}${s.metadata.name}`,
    label: s.metadata.name,
    description,
    iconId,
    sandbox: s,
  };
}

/** Message user-facing partagé : serveur vanyline non démarré (supervisor.current()
 *  === undefined). Identique à SERVER_NOT_STARTED dans extension.ts. */
export const SERVER_NOT_STARTED =
  'VNL-EXT-021: serveur vanyline non démarré (voir vanyline.restartServer)';

function errorNode(err: unknown): ResourceNode {
  const { message } = mapRpcError(err);
  return {
    kind: 'error',
    id: `error:${message}`,
    label: message,
    iconId: 'error',
  };
}

function serverNotStartedNode(): ResourceNode {
  return {
    kind: 'info',
    id: 'info:VNL-EXT-021',
    label: SERVER_NOT_STARTED,
    iconId: 'info',
  };
}
