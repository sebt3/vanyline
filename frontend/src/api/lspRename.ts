import { Text } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import { LSPPlugin } from '@codemirror/lsp-client';
import type { LSPClient } from '@codemirror/lsp-client';
import type { SandboxFsClient } from './sandboxWs';

export interface LspPosition { line: number; character: number }
export interface LspRange { start: LspPosition; end: LspPosition }
export interface TextEdit { range: LspRange; newText: string }
export interface WorkspaceEdit {
  changes?: Record<string, TextEdit[]>;
  documentChanges?: { textDocument: { uri: string }; edits: TextEdit[] }[];
}

/** position LSP {line, character} → offset dans un Text CodeMirror. `line` et
 *  `character` clampés (offsets UTF-16, cohérents avec les offsets CodeMirror).
 *  `line` hors limites → `doc.length`. */
export function positionToOffset(doc: Text, pos: LspPosition): number {
  if (pos.line >= doc.lines) return doc.length;
  const line = doc.line(Math.max(1, pos.line + 1));
  const character = Math.max(0, Math.min(pos.character, line.text.length));
  return line.from + character;
}

/** offset → position LSP {line, character} dans un Text (0-based). */
export function offsetToPosition(doc: Text, offset: number): LspPosition {
  const clamped = Math.max(0, Math.min(offset, doc.length));
  const line = doc.lineAt(clamped);
  return { line: line.number - 1, character: clamped - line.from };
}

/** position LSP → offset dans une chaîne (offsets UTF-16, cohérents avec les
 *  offsets CodeMirror). `line` clampée à la dernière ligne, `character` clampé à la
 *  longueur de ligne. */
export function stringPositionToOffset(content: string, pos: LspPosition): number {
  const lines = content.split('\n');
  const line = Math.max(0, Math.min(pos.line, lines.length - 1));
  const lineText = lines[line];
  const character = Math.max(0, Math.min(pos.character, lineText.length));
  let offset = 0;
  for (let i = 0; i < line; i++) offset += lines[i].length + 1; // +1 newline
  return offset + character;
}

/** Applique des TextEdit à une chaîne : offsets calculés sur le contenu d'origine,
 *  tri décroissant par `start`, puis `replace_range` séquentiel (miroir
 *  `apply_text_edits` de la sandbox). */
export function applyTextEditsToString(content: string, edits: TextEdit[]): string {
  const parsed = edits
    .map((e) => ({
      start: stringPositionToOffset(content, e.range.start),
      end: stringPositionToOffset(content, e.range.end),
      newText: e.newText,
    }))
    .sort((a, b) => b.start - a.start);
  let out = content;
  for (const e of parsed) out = out.slice(0, e.start) + e.newText + out.slice(e.end);
  return out;
}

/** Extrait les `(uri, edits)` d'un `WorkspaceEdit` : `changes` (map uri → TextEdit[])
 *  puis `documentChanges` (array de `{ textDocument: { uri }, edits }`), déduplication
 *  par URI (les edits d'une même URI sont concaténés), edits vides ignorés (miroir
 *  `workspace_edit_files` de la sandbox). */
export function workspaceEditFiles(edit: WorkspaceEdit): { uri: string; edits: TextEdit[] }[] {
  const result: { uri: string; edits: TextEdit[] }[] = [];
  const seen = new Set<string>();
  for (const [uri, edits] of Object.entries(edit.changes ?? {})) {
    if (!edits.length) continue;
    seen.add(uri);
    result.push({ uri, edits });
  }
  for (const dc of edit.documentChanges ?? []) {
    const uri = dc.textDocument.uri;
    if (!dc.edits.length) continue;
    const entry = result.find((r) => r.uri === uri);
    if (entry) entry.edits = entry.edits.concat(dc.edits);
    else { seen.add(uri); result.push({ uri, edits: dc.edits }); }
  }
  return result;
}

/** `file:///<relatif>` → `<relatif>` (chemin pour `/ws/fs`). */
export function uriToPath(uri: string): string {
  return uri.slice('file:///'.length);
}

/** Flux rename custom : requête `textDocument/rename` directe (API publique du client),
 *  application du `WorkspaceEdit` nous-mêmes — fichiers ouverts (via
 *  `client.workspace.getFile(uri)?.getView()`) par transaction CodeMirror
 *  (`{ changes, userEvent: 'rename' }`), fichiers non ouverts par `read` (raw) puis
 *  `applyTextEditsToString` puis `write` de `/ws/fs`, un par un. Séquentiel, best-effort :
 *  un fichier en échec → `failed`, on continue (pas de rollback). Throw si la requête
 *  échoue (renommage impossible). Retourne les chemins `applied` et `failed`. */
export async function renameSymbolCustom(
  client: LSPClient,
  view: EditorView,
  uri: string,
  pos: number,
  fsClient: SandboxFsClient,
  newName: string,
): Promise<{ applied: string[]; failed: string[] }> {
  client.sync();
  const edit = await client.request<{
    textDocument: { uri: string };
    position: LspPosition;
    newName: string;
  }, WorkspaceEdit | null>(
    'textDocument/rename',
    {
      textDocument: { uri },
      position: offsetToPosition(view.state.doc, pos),
      newName,
    },
  );
  if (!edit) return { applied: [], failed: [] };

  const applied: string[] = [];
  const failed: string[] = [];
  for (const { uri: fileUri, edits } of workspaceEditFiles(edit)) {
    const path = uriToPath(fileUri);
    const file = client.workspace.getFile(fileUri);
    const fileView = file?.getView() ?? null;
    if (fileView) {
      try {
        fileView.dispatch({
          changes: edits.map((e) => ({
            from: positionToOffset(fileView.state.doc, e.range.start),
            to: positionToOffset(fileView.state.doc, e.range.end),
            insert: e.newText,
          })),
          userEvent: 'rename',
        });
        applied.push(path);
      } catch (err) {
        failed.push(path);
      }
    } else {
      try {
        const resp = await fsClient.request<{ ok: boolean; content: string }>('read', {
          path,
          raw: true,
        });
        const content = applyTextEditsToString(resp.content, edits);
        await fsClient.request<{ ok: boolean }>('write', { path, content });
        applied.push(path);
      } catch (err) {
        failed.push(path);
      }
    }
  }
  return { applied, failed };
}

/** Commande depuis une vue éditeur : extrait le mot sous le curseur
 *  (`view.state.wordAt(view.state.selection.main.head)`), demande le nouveau nom
 *  (`window.prompt`, défaut = mot courant), appelle `renameSymbolCustom`. Retourne un
 *  message de statut humain ('' si annulé / pas de plugin / pas de mot). */
export async function renameSymbolFromView(
  view: EditorView,
  fsClient: SandboxFsClient,
): Promise<string> {
  const plugin = LSPPlugin.get(view);
  if (!plugin) return '';
  const word = view.state.wordAt(view.state.selection.main.head);
  if (!word) return '';
  const current = view.state.sliceDoc(word.from, word.to);
  const newName = window.prompt('Nouveau nom du symbole', current);
  if (!newName) return '';
  const res = await renameSymbolCustom(
    plugin.client, view, plugin.uri, word.from, fsClient, newName,
  );
  if (res.applied.length === 0 && res.failed.length === 0) return 'Aucun renommage';
  const msg = `Renommé dans ${res.applied.length} fichier(s) : ${res.applied.join(', ')}`;
  return res.failed.length ? `${msg} ; Échecs : ${res.failed.join(', ')}` : msg;
}