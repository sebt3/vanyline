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

/* ================================================================== *
 * Commandes create/delete (tache 02) : purete testable sous vitest.   *
 * La colle vscode (adaptateur PromptApi, enregistrement des commandes) *
 * vit dans panels/resources.ts.                                       *
 * ================================================================== */

/** Noms de ressource K8s = labels RFC1123 (longueur 1-63). */
export const NAME_MAX = 63;

const NAME_PATTERN = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
/** `<schema>://...` (RFC 3986, avec une etendue non-bleue après « :// ») ou scp `usr@host:chemin`. */
const URL_SCHEMA = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^\s]+$/u;
const URL_SCP = /^[^:@/\s]+@[^:/\s]+:.+$/;

/** Rejet de saisie de surface : identifiant unique de projet (règle AGENTS.md). */
const surfaceError = (detail: string): string => `VNL-EXT-026: ${detail}`;

/* « aucun Owner disponible » : message VNL-EXT-027 (création d'owner par CLI). */
const NO_OWNER =
  'VNL-EXT-027: aucun Owner dans le namespace - créez-le d’abord avec la CLI (`vanyline owner create <nom>`)';
/* « aucun Project disponible » : message VNL-EXT-027. */
const NO_PROJECT =
  'VNL-EXT-027: aucun Project disponible - créez-le d’abord (vanyline.project.create)';
/* « aucune Sandbox disponible » (delete sans cible) : message VNL-EXT-027. */
const NO_SANDBOX =
  'VNL-EXT-027: aucune Sandbox dans le namespace - rien à supprimer';

/** Portes de dialogue injectées (vscode dans panels/resources.ts, objets faux dans les tests).
 *  `undefined` sur toute porte ⇒ annulation utilisateur. */
export interface PromptApi {
  input(opts: {
    prompt: string;
    value?: string;
    validate?: (v: string) => string | undefined;
  }): Promise<string | undefined>;
  pick<T extends { label: string; description?: string }>(
    title: string,
    items: T[],
  ): Promise<T | undefined>;
  /** true seulement si l'utilisateur clique le bouton explicite de confirmation. */
  confirm(message: string): Promise<boolean>;
}

export interface RunResult {
  readonly ok: boolean;
  /** Annulation utilisateur : ok=false ET cancelled=true (le panneau ne doit rien afficher). */
  readonly cancelled?: boolean;
  /** Message user-facing (français) : succès court, ou erreur avec code VNL-XXX. */
  readonly message: string;
}

/** Résultat d'une liste de sélection : soit la liste, soit le RunResult d'échec.
 *  Convention partagée avec le modèle de la tâche 01 : un échec RPC ne remonte
 *  JAMAIS en rejet hors des run* (le handler de commande n'a pas de catch). */
type ListOutcome<T> = { readonly list: T[] } | { readonly fail: RunResult };

async function listFor<T>(rpc: RpcLike, method: string): Promise<ListOutcome<T>> {
  try {
    const objs = await rpc.request<T[]>(method);
    return { list: Array.isArray(objs) ? objs : [] };
  } catch (err) {
    return {
      fail: { ok: false, message: `VNL-EXT-025: ${method} (${mapRpcError(err).message})` },
    };
  }
}

/** Nom de ressource K8s valide (RFC1123 label, max 63) ; message `VNL-EXT-026` sinon. */
export function validateResourceName(name: string): string | undefined {
  if (name.length === 0 || name.length > NAME_MAX) {
    return surfaceError('nom de ressource invalide (label RFC1123, 1-63 caractères)');
  }
  return NAME_PATTERN.test(name)
    ? undefined
    : surfaceError('nom de ressource invalide (minuscules, chiffres, tirets)');
}

/** URL du dépôt : protochem `<schema>://...` ou scp `usr@host:chemin` ; `VNL-EXT-026` sinon. */
export function validateRepoUrl(url: string): string | undefined {
  if (url.length === 0) {
    return surfaceError('URL du dépôt requise');
  }
  return URL_SCHEMA.test(url) || URL_SCP.test(url)
    ? undefined
    : surfaceError('forme d’URL invalide (https://… ou git@host:…)');
}

/** Ref git (branche) : non vide, sans espace ni contrôle, sans « .. », ne commence pas par « - » */
export function validateGitRef(ref: string): string | undefined {
  if (ref.length === 0) {
    return surfaceError('branche requise');
  }
  if (/\s/.test(ref) || /[\u0000-\u001f]/.test(ref) || ref.includes('..') || ref.startsWith('-')) {
    return surfaceError('branche invalide (espace, « .. » ou tiret en tête interdits)');
  }
  return undefined;
}

/** Flux `vanyline.project.create`. owner = hint (nœud owner) ou QuickPick sur owners/list. */
export async function runProjectCreate(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  ownerHint?: ResourceNode,
): Promise<RunResult> {
  if (rpc === undefined) {
    return { ok: false, message: SERVER_NOT_STARTED };
  }

  let owner: string;
  if (ownerHint?.kind === 'owner') {
    owner = ownerHint.label;
  } else {
    const outcome = await listFor<VnlOwner>(rpc, 'owners/list');
    if ('fail' in outcome) {
      return outcome.fail;
    }
    if (outcome.list.length === 0) {
      return { ok: false, message: NO_OWNER };
    }
    const picked = await ui.pick(
      'Owner',
      outcome.list.map((o) => ({ label: o.metadata.name })),
    );
    if (picked === undefined) {
      return { ok: false, cancelled: true, message: '' };
    }
    owner = picked.label;
  }

  const name = await ui.input({
    prompt: 'Nom du projet',
    value: '',
    validate: validateResourceName,
  });
  if (name === undefined) {
    return { ok: false, cancelled: true, message: '' };
  }
  const nameError = validateResourceName(name);
  if (nameError !== undefined) {
    return { ok: false, message: nameError };
  }

  const repoUrl = await ui.input({
    prompt: "URL du dépôt (https://… ou git@hôte:…)",
    value: '',
    validate: validateRepoUrl,
  });
  if (repoUrl === undefined) {
    return { ok: false, cancelled: true, message: '' };
  }
  const repoError = validateRepoUrl(repoUrl);
  if (repoError !== undefined) {
    return { ok: false, message: repoError };
  }

  // branche par défaut optionnelle : vide/absent ⇒ champ omis ; valeur initiale 'main'.
  const defaultBranch = await ui.input({
    prompt: 'Branche par défaut (optionnel)',
    value: 'main',
    validate: validateGitRef,
  });
  if (defaultBranch === undefined) {
    return { ok: false, cancelled: true, message: '' };
  }
  const params: Record<string, unknown> = { name, owner, repoUrl };
  if (defaultBranch !== '') {
    const branchError = validateGitRef(defaultBranch);
    if (branchError !== undefined) {
      return { ok: false, message: branchError };
    }
    params.defaultBranch = defaultBranch;
  }

  try {
    await rpc.request('projects/create', params);
    return { ok: true, message: `Projet ${name} créé` };
  } catch (err) {
    return { ok: false, message: `VNL-EXT-025: projects/create (${mapRpcError(err).message})` };
  }
}

/** Flux `vanyline.project.delete`. projet = hint (nœud project) ou QuickPick sur projects/list. */
export async function runProjectDelete(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  targetHint?: ResourceNode,
): Promise<RunResult> {
  if (rpc === undefined) {
    return { ok: false, message: SERVER_NOT_STARTED };
  }

  let name: string;
  if (targetHint?.kind === 'project') {
    name = targetHint.label;
  } else {
    const outcome = await listFor<VnlProject>(rpc, 'projects/list');
    if ('fail' in outcome) {
      return outcome.fail;
    }
    if (outcome.list.length === 0) {
      return { ok: false, message: NO_PROJECT };
    }
    const picked = await ui.pick(
      'Projet',
      outcome.list.map((p) => ({ label: p.metadata.name, description: p.spec?.owner ?? '' })),
    );
    if (picked === undefined) {
      return { ok: false, cancelled: true, message: '' };
    }
    name = picked.label;
  }

  const confirmed = await ui.confirm(`Supprimer le projet « ${name} » ?`);
  if (!confirmed) {
    return { ok: false, cancelled: true, message: '' };
  }

  try {
    await rpc.request('projects/delete', { name });
    return { ok: true, message: `Projet ${name} supprimé` };
  } catch (err) {
    return { ok: false, message: `VNL-EXT-025: projects/delete (${mapRpcError(err).message})` };
  }
}

/** Flux `vanyline.sandbox.create`. projet = hint (nœud project) ou QuickPick sur projects/list. */
export async function runSandboxCreate(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  projectHint?: ResourceNode,
): Promise<RunResult> {
  if (rpc === undefined) {
    return { ok: false, message: SERVER_NOT_STARTED };
  }

  let project: string;
  if (projectHint?.kind === 'project') {
    project = projectHint.label;
  } else {
    const outcome = await listFor<VnlProject>(rpc, 'projects/list');
    if ('fail' in outcome) {
      return outcome.fail;
    }
    if (outcome.list.length === 0) {
      return { ok: false, message: NO_PROJECT };
    }
    const picked = await ui.pick(
      'Projet',
      outcome.list.map((p) => ({ label: p.metadata.name, description: p.spec?.owner ?? '' })),
    );
    if (picked === undefined) {
      return { ok: false, cancelled: true, message: '' };
    }
    project = picked.label;
  }

  const name = await ui.input({
    prompt: 'Nom de la sandbox',
    value: '',
    validate: validateResourceName,
  });
  if (name === undefined) {
    return { ok: false, cancelled: true, message: '' };
  }
  const nameError = validateResourceName(name);
  if (nameError !== undefined) {
    return { ok: false, message: nameError };
  }

  const branch = await ui.input({
    prompt: 'Branche',
    value: 'main',
    validate: validateGitRef,
  });
  if (branch === undefined) {
    return { ok: false, cancelled: true, message: '' };
  }
  const branchError = validateGitRef(branch);
  if (branchError !== undefined) {
    return { ok: false, message: branchError };
  }

  try {
    await rpc.request('sandboxes/create', { name, project, branch });
    return { ok: true, message: `Sandbox ${name} créée` };
  } catch (err) {
    return { ok: false, message: `VNL-EXT-025: sandboxes/create (${mapRpcError(err).message})` };
  }
}

/** Flux `vanyline.sandbox.delete`. miroir de runProjectDelete (items : status?.phase). */
export async function runSandboxDelete(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  targetHint?: ResourceNode,
): Promise<RunResult> {
  if (rpc === undefined) {
    return { ok: false, message: SERVER_NOT_STARTED };
  }

  let name: string;
  if (targetHint?.kind === 'sandbox') {
    name = targetHint.label;
  } else {
    const outcome = await listFor<VnlSandbox>(rpc, 'sandboxes/list');
    if ('fail' in outcome) {
      return outcome.fail;
    }
    if (outcome.list.length === 0) {
      return { ok: false, message: NO_SANDBOX };
    }
    const picked = await ui.pick(
      'Sandbox',
      outcome.list.map((s) => ({
        label: s.metadata.name,
        description: s.status?.phase ?? 'phase inconnue',
      })),
    );
    if (picked === undefined) {
      return { ok: false, cancelled: true, message: '' };
    }
    name = picked.label;
  }

  const confirmed = await ui.confirm(`Supprimer la sandbox « ${name} » ?`);
  if (!confirmed) {
    return { ok: false, cancelled: true, message: '' };
  }

  try {
    await rpc.request('sandboxes/delete', { name });
    return { ok: true, message: `Sandbox ${name} supprimée` };
  } catch (err) {
    return { ok: false, message: `VNL-EXT-025: sandboxes/delete (${mapRpcError(err).message})` };
  }
}

/** Flux `vanyline.sandbox.stop`. Réversible — PAS de modale (transition). */
export async function runSandboxStop(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  targetHint?: ResourceNode,
): Promise<RunResult> {
  return runSandboxTransition(rpc, ui, 'stop', targetHint);
}

/** Flux `vanyline.sandbox.start`. Réversible — PAS de modale (transition). */
export async function runSandboxStart(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  targetHint?: ResourceNode,
): Promise<RunResult> {
  return runSandboxTransition(rpc, ui, 'start', targetHint);
}

/** Transition stop/start partagée : `sandboxes/{stop,start}` avec `{ name }`,
 *  PAS de confirmation (transition réversible — seule la suppression est destructive,
 *  cf. runSandboxDelete). Cible = hint de nœud sandbox, ou sélection
 *  (sandboxes/list + pick ⇒ NO_SANDBOX si liste vide ; échec de liste via listFor
 *  ⇒ VNL-EXT-025). Échec RPC ⇒ VNL-EXT-025, jamais de rejet hors du run. */
async function runSandboxTransition(
  rpc: RpcLike | undefined,
  ui: PromptApi,
  action: 'stop' | 'start',
  targetHint?: ResourceNode,
): Promise<RunResult> {
  if (rpc === undefined) {
    return { ok: false, message: SERVER_NOT_STARTED };
  }

  let name: string;
  if (targetHint?.kind === 'sandbox') {
    name = targetHint.label;
  } else {
    const outcome = await listFor<VnlSandbox>(rpc, 'sandboxes/list');
    if ('fail' in outcome) {
      return outcome.fail;
    }
    if (outcome.list.length === 0) {
      return { ok: false, message: NO_SANDBOX };
    }
    const picked = await ui.pick(
      'Sandbox',
      outcome.list.map((s) => ({
        label: s.metadata.name,
        description: s.status?.phase ?? 'phase inconnue',
      })),
    );
    if (picked === undefined) {
      return { ok: false, cancelled: true, message: '' };
    }
    name = picked.label;
  }

  try {
    await rpc.request(`sandboxes/${action}`, { name });
    return {
      ok: true,
      message: `Sandbox ${name} ${action === 'stop' ? 'arrêtée' : 'démarrée'}`,
    };
  } catch (err) {
    return {
      ok: false,
      message: `VNL-EXT-025: sandboxes/${action} (${mapRpcError(err).message})`,
    };
  }
}
