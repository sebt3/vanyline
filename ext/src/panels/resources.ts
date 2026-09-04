import * as vscode from 'vscode';
import { createResourcesModel, type ResourceNode, type RpcLike } from '../resources';
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

  let treeView: vscode.TreeView<ResourceNode> | undefined;

  /** Rafraîchit la vue (appel public de l'API 1.67+, mais ABSENT des types
   *  `@types/vscode` installés — on l'appelle quand même : le contrat le veut). */
  function refreshView(): void {
    (treeView as unknown as { refresh(): void }).refresh();
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
      if (node === undefined) {
        // Racine : modèle courant, ou « non démarré » s'il est absent (vue detach).
        return model?.getRoots() ?? createResourcesModel(undefined).getRoots();
      }
      const current = model;
      return current ? current.getChildren(node) : Promise.resolve<ResourceNode[]>([]);
    },
  };

  treeView = vscode.window.createTreeView('vanyline.resources', { treeDataProvider });
  context.subscriptions.push(treeView);

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.resources.refresh', async (): Promise<void> => {
      refreshView();
      updateTitle();
    }),
  );

  function attachServer(conn: RpcLike): void {
    model = createResourcesModel(conn, (line): void => {
      channel.appendLine(line);
    });
    refreshView();
    updateTitle();
  }

  function detachServer(): void {
    model = undefined;
    refreshView();
    updateTitle();
  }

  return { attachServer, detachServer };
}
