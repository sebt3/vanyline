import * as vscode from 'vscode';
import {
  createResourcesModel,
  runProjectCreate,
  runProjectDelete,
  runSandboxCreate,
  runSandboxDelete,
  runSandboxStart,
  runSandboxStop,
  type PromptApi,
  type ResourceNode,
  type RunResult,
  type RpcLike,
} from '../resources';
import type { LogChannel } from '../rpc';

export interface ResourcesView {
  attachServer(conn: RpcLike): void;
  detachServer(): void;
}

/** Enregistre la vue native « Resources » (Owners > Projects > Sandboxes).
 *
 * La logique pure (appel RPC K8s, filtrage, icônes par phase) vit dans
 * `resources.ts` — ce panneau n'est que de la colle vscode : createTreeView,
 * mapping ResourceNode → TreeItem, refresh manuel + titre/description de vue. */
export function registerResources(
  context: vscode.ExtensionContext,
  channel: LogChannel,
): ResourcesView {
  // Modèle courant (source de données de l'arbre). Réassigné par attach/detach
  // selon le handle du superviseur — aucune donnée n'est mise en cache.
  let model: ReturnType<typeof createResourcesModel> | undefined;

  /** Connexion RPC courante (source des commandes create/delete). Réassignée par
   *  attach/detach — `undefined` hors d'un handle vivant ⇒ repli QuickPick, pas d'appel. */
  let rpc: RpcLike | undefined;

  let treeView: vscode.TreeView<ResourceNode> | undefined;

  /** Rafraîchit la vue — retourne le Thenable pour que l'appelant puisse chaîner
   *  `updateTitle()` APRES le refresh (le namespace résolu n'est à jour qu'une fois
   *  les appels RPC du refresh terminés). API publique de l'extension host depuis
   *  1.67 mais ABSENTE des types `@types/vscode` installés (vérifié 1.136.0,
   *  index.d.ts:12149-12236) — d'où le cast isolé, seul endroit qui l'utilise. */
  function refreshView(): Promise<void> {
    if (treeView === undefined) {
      return Promise.resolve();
    }
    return (treeView as unknown as { refresh(): Promise<void> }).refresh();
  }

  /** Refresh puis titre/description — chemin des commandes et de attach/detach.
   *  `updateTitle` aussi en onRejected : le modèle convertit toute erreur RPC en
   *  nœud error (le provider ne rejette jamais), un rejet de refresh() lui-même
   *  n'est pas attendu — on met quand même le titre à jour plutôt que de laisser
   *  une promesse non gérée. */
  function refreshAndView(): Promise<void> {
    return refreshView().then(updateTitle, updateTitle);
  }

  /** `title` + `description` de la vue à jour après tout refresh (attach/detach/
   *  commande) : description = namespace résolu, absent ⇒ description retirée. */
  function updateTitle(): void {
    if (treeView === undefined) {
      return;
    }
    treeView.title = 'Resources';
    treeView.description = model?.namespace() ?? '';
  }

  const treeDataProvider: vscode.TreeDataProvider<ResourceNode> = {
    getTreeItem(node: ResourceNode): vscode.TreeItem {
      const item = new vscode.TreeItem(node.label);
      item.id = node.id;
      item.description = node.description;
      item.contextValue = node.kind;
      item.iconPath = node.iconId ? new vscode.ThemeIcon(node.iconId) : undefined;
      item.collapsibleState =
        node.kind === 'owner' || node.kind === 'project'
          ? vscode.TreeItemCollapsibleState.Collapsed
          : vscode.TreeItemCollapsibleState.None;
      return item;
    },

    getChildren(node?: ResourceNode): Promise<ResourceNode[]> {
      // Modèle « non démarré » quand aucun handle n'est vivant : mêmes sémantiques
      // à la racine (nœud info) qu'en enfant (nœud error « serveur non démarré »).
      const current = model ?? createResourcesModel(undefined);
      return node === undefined ? current.getRoots() : current.getChildren(node);
    },
  };

  treeView = vscode.window.createTreeView('vanyline.resources', { treeDataProvider });
  context.subscriptions.push(treeView);

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.resources.refresh', async (): Promise<void> => {
      await refreshView();
      updateTitle();
    }),
  );

  /** Adaptateur PromptApi : colle les portes de dialogue vscode sur la logique pure. */
  const ui: PromptApi = {
    input: (opts) =>
      Promise.resolve(
        vscode.window.showInputBox({
          prompt: opts.prompt,
          value: opts.value ?? '',
          validateInput: opts.validate ?? (() => undefined),
        }),
      ),
    pick: (title, items) =>
      Promise.resolve(vscode.window.showQuickPick(items, { title })),
    confirm: async (message) =>
      (await vscode.window.showWarningMessage(message, { modal: true }, 'Supprimer')) ===
      'Supprimer',
  };

  /** Enregistre une commande create/delete : reçoit le nœud de l'arbre (menu contextuel)
   *  en 1ʳᵉ arg ; la palette renvoie `undefined` ⇒ repli QuickPick dans le flux. Une fois
   *  l'action réussie, refresh + titre de vue puis message user-facing (français). */
  function registerRun(
    command: string,
    run: (node: ResourceNode | undefined) => Promise<RunResult>,
  ): void {
    context.subscriptions.push(
      vscode.commands.registerCommand(command, async (node?: ResourceNode): Promise<void> => {
        const r = await run(node);
        if (r.cancelled) return;
        if (r.ok) {
          await refreshAndView();
          void vscode.window.showInformationMessage(`vanyline: ${r.message}`);
        } else {
          void vscode.window.showErrorMessage(`vanyline: ${r.message}`);
        }
      }),
    );
  }

  registerRun('vanyline.project.create', (n) => runProjectCreate(rpc, ui, n));
  registerRun('vanyline.project.delete', (n) => runProjectDelete(rpc, ui, n));
  registerRun('vanyline.sandbox.create', (n) => runSandboxCreate(rpc, ui, n));
  registerRun('vanyline.sandbox.delete', (n) => runSandboxDelete(rpc, ui, n));
  registerRun('vanyline.sandbox.stop', (n) => runSandboxStop(rpc, ui, n));
  registerRun('vanyline.sandbox.start', (n) => runSandboxStart(rpc, ui, n));

  function attachServer(conn: RpcLike): void {
    rpc = conn;
    model = createResourcesModel(conn, (line): void => {
      channel.appendLine(line);
    });
    void refreshAndView();
  }

  function detachServer(): void {
    model = undefined;
    rpc = undefined;
    void refreshAndView();
  }

  return { attachServer, detachServer };
}
