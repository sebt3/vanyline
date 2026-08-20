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

  constructor(client: LSPClient) { super(client); }

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
}
