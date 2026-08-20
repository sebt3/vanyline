import { Text, type ChangeSet } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import { LSPPlugin, Workspace, type WorkspaceFile } from '@codemirror/lsp-client';
import type { LSPClient } from '@codemirror/lsp-client';

/// Implémentation privée de `WorkspaceFile` — un seul view par fichier (un panel
/// Editor par fichier dans notre IDE).
class VanylineWorkspaceFile implements WorkspaceFile {
  readonly uri: string;
  readonly languageId: string;
  version: number;
  doc: Text;
  readonly view: EditorView;

  constructor(uri: string, languageId: string, version: number, doc: Text, view: EditorView) {
    this.uri = uri;
    this.languageId = languageId;
    this.version = version;
    this.doc = doc;
    this.view = view;
  }
  getView() { return this.view; }
}

/// Workspace LSP custom : reproduit `DefaultWorkspace` (comportement lu dans
/// node_modules/@codemirror/lsp-client/src/workspace.ts) — tracking des fichiers
/// ouverts, syncFiles, didOpen/didClose. Le package n'exporte pas `DefaultWorkspace` :
/// on réimplémente son comportement, pour pouvoir surcharger `updateFile` (fichiers
/// non ouverts) dans la tâche lsp-rename-cross-file.
export class VanylineWorkspace extends Workspace {
  files: WorkspaceFile[] = [];
  private fileVersions: Record<string, number> = {};
  private openFileCallback?: (path: string) => void;

  constructor(client: LSPClient, openFileCallback?: (path: string) => void) {
    super(client);
    this.openFileCallback = openFileCallback;
  }

  private nextFileVersion(uri: string): number {
    return (this.fileVersions[uri] = (this.fileVersions[uri] ?? -1) + 1);
  }

  syncFiles(): { file: WorkspaceFile; prevDoc: Text; changes: ChangeSet }[] {
    const result: { file: WorkspaceFile; prevDoc: Text; changes: ChangeSet }[] = [];
    for (const file of this.files) {
      const plugin = LSPPlugin.get((file as VanylineWorkspaceFile).view);
      if (!plugin) continue;
      const changes = plugin.unsyncedChanges;
      if (!changes.empty) {
        result.push({ changes, file, prevDoc: file.doc });
        file.doc = (file as VanylineWorkspaceFile).view.state.doc;
        file.version = this.nextFileVersion(file.uri);
        plugin.clear();
      }
    }
    return result;
  }

  openFile(uri: string, languageId: string, view: EditorView): void {
    if (this.getFile(uri)) {
      throw new Error('VanylineWorkspace does not support multiple views on the same file');
    }
    const file = new VanylineWorkspaceFile(
      uri, languageId, this.nextFileVersion(uri), view.state.doc, view,
    );
    this.files.push(file);
    this.client.didOpen(file);
  }

  closeFile(uri: string): void {
    const file = this.getFile(uri);
    if (file) {
      this.files = this.files.filter((f) => f !== file);
      this.client.didClose(uri);
    }
  }

  /** Appelé par `jumpToDefinition`/`jumpToOrigin` du package quand la cible d'une
   *  navigation (go-to-definition, references, ...) est dans un fichier différent du
   *  fichier courant — sans cette méthode le résultat revient bien (LSP round-trip
   *  correct) mais rien ne se passe dans l'UI, trouvé en usage réel : le package
   *  délègue entièrement l'ouverture/bascule d'onglet au workspace applicatif, il n'y
   *  a pas de comportement par défaut côté package qui ouvre un fichier inconnu.
   *
   *  Fichier déjà suivi → son view directement. Sinon, déclenche l'ouverture d'un
   *  onglet via le callback `open-file` d'`IdeShell` (même mécanisme que l'Explorer)
   *  et attend (poll borné) que l'`Editor.vue` nouvellement monté charge le fichier
   *  et s'enregistre dans CE workspace via `openFile` (appelé par le plugin LSP à
   *  l'attache du CodeMirror view — pas directement par nous, pas de lien promesse
   *  entre la création du panel dockview et cet enregistrement).
   *
   *  Limite connue et acceptée : ne gère que les cibles à l'intérieur du workspace.
   *  Une définition dans la bibliothèque standard (montée en volume toolchain,
   *  ex. `/toolchains/rust/.../option.rs`) traverse le bridge WS sous la même forme
   *  syntaxique `file:///<chemin>` qu'un vrai fichier relatif du workspace (le bridge
   *  ne réécrit que les URIs sous `sandbox_root`, cf. `sandbox/src/ws/lsp.rs`) — rien
   *  ne les distingue explicitement côté client. Heuristique de repli : un chemin
   *  commençant par `toolchains/` est traité comme hors workspace (aucune raison
   *  qu'un vrai fichier de projet vive à cet endroit) → pas d'ouverture tentée,
   *  navigation silencieusement no-op pour ce cas précis. Visualisation en lecture
   *  seule de fichiers hors workspace : pas construite, nécessiterait un schéma
   *  d'URI distinct et un endpoint de lecture propre (hors `/ws/fs`, confiné à
   *  `sandbox_root`) — pas fait ici, périmètre distinct.
   */
  async displayFile(uri: string): Promise<EditorView | null> {
    const existing = this.getFile(uri) as VanylineWorkspaceFile | null;
    if (existing) return existing.view;

    const path = uri.startsWith('file:///') ? uri.slice('file:///'.length) : null;
    if (path === null || path.startsWith('toolchains/') || !this.openFileCallback) return null;

    this.openFileCallback(path);

    for (let i = 0; i < 100; i++) {
      const file = this.getFile(uri) as VanylineWorkspaceFile | null;
      if (file) return file.view;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return null;
  }
}
