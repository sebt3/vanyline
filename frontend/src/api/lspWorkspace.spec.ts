import { Text } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import { LSPPlugin, type WorkspaceFile } from '@codemirror/lsp-client';
import type { LSPClient } from '@codemirror/lsp-client';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { VanylineWorkspace } from './lspWorkspace';

/** Faux client LSP casté en `LSPClient`. */
function makeFakeClient(): LSPClient {
  return {
    didOpen: vi.fn(),
    didClose: vi.fn(),
    sync: vi.fn(),
  } as unknown as LSPClient;
}

/** Faux view avec un doc `Text.of(['abc'])` et un `dispatch` spy. */
function makeFakeView(): EditorView {
  return {
    state: { doc: Text.of(['abc']) },
    dispatch: vi.fn(),
  } as unknown as EditorView;
}

/** Spies reset après chaque test. */
beforeEach(() => {
  vi.restoreAllMocks();
});

describe('VanylineWorkspace', () => {
  it('openFile tracks le fichier et appelle didOpen', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView();

    ws.openFile('file:///src/main.rs', 'rust', view);

    expect(ws.files.length).toBe(1);
    const openedFile = ws.files[0];
    expect(openedFile.uri).toBe('file:///src/main.rs');
    expect((openedFile as WorkspaceFile).languageId).toBe('rust');
    expect(openedFile.version).toBe(0);
    expect(openedFile.getView()).toBe(view);
    expect(client.didOpen).toHaveBeenCalledWith(openedFile);
  });

  it('openFile throw si même uri deux fois', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView();

    ws.openFile('file:///src/main.rs', 'rust', view);
    expect(() => ws.openFile('file:///src/main.rs', 'rust', makeFakeView()))
      .toThrow('VanylineWorkspace does not support multiple views on the same file');
  });

  it('getFile retourne le fichier tracked', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView();

    ws.openFile('file:///src/main.rs', 'rust', view);

    expect(ws.getFile('file:///src/main.rs')).not.toBeNull();
    expect(ws.getFile('inconnu')).toBeNull();
  });

  it('closeFile retire le fichier et appelle didClose', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView();
    const uri = 'file:///src/main.rs';

    ws.openFile(uri, 'rust', view);
    expect(ws.files.length).toBe(1);

    ws.closeFile(uri);

    expect(ws.files.length).toBe(0);
    expect(client.didClose).toHaveBeenCalledWith(uri);
  });

  it('closeFile uri inconnu n\'appelle pas didClose', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);

    ws.closeFile('file:///inconnu');

    expect(client.didClose).not.toHaveBeenCalled();
    expect(ws.files.length).toBe(0);
  });

  it('updateFile dispatche vers le view ouvert', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView() as EditorView;

    ws.openFile('file:///src/main.rs', 'rust', view);

    const update = { changes: { changes: [] as any } };
    (ws as any).updateFile('file:///src/main.rs', update);

    expect(view.dispatch).toHaveBeenCalled();
  });

  it('syncFiles retourne [] sans plugin', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const view = makeFakeView();

    ws.openFile('file:///src/main.rs', 'rust', view);

    vi.spyOn(LSPPlugin, 'get').mockReturnValue(null);

    const result = ws.syncFiles();

    expect(result).toEqual([]);
  });

  it('syncFiles rapport les changements du plugin', () => {
    const client = makeFakeClient();
    const ws = new VanylineWorkspace(client);
    const oldDoc = Text.of(['abc']);
    const newDoc = Text.of(['xyz']);
    const dispatchSpy = vi.fn();
    // View avec doc initial puis modifiable via getter
    const view = {
      state: { doc: oldDoc },
      dispatch: dispatchSpy,
    } as unknown as EditorView;

    ws.openFile('file:///src/main.rs', 'rust', view);

    const clearSpy = vi.fn();
    vi.spyOn(LSPPlugin, 'get').mockReturnValue({
      unsyncedChanges: { empty: false },
      clear: clearSpy,
    } as any);

    // Modifier le doc dans le view (simule la modification utilisateur)
    // Le workspace doit voir le nouveau doc via view.state.doc au moment de syncFiles
    // Pour cela, on remplace le getter state.doc
    Object.defineProperty(view, 'state', {
      value: { doc: newDoc },
      configurable: true,
    });

    const previousFileVersion = ws.files[0].version;

    const result = ws.syncFiles();

    expect(result.length).toBe(1);
    expect(result[0].file.uri).toBe('file:///src/main.rs');
    expect(result[0].prevDoc).toBe(oldDoc);
    expect(result[0].file.doc).toBe(newDoc);
    expect(ws.files[0].version).toBe(previousFileVersion + 1);
    expect(clearSpy).toHaveBeenCalled();
  });

  describe('displayFile', () => {
    it('fichier déjà ouvert : rend son view directement, pas de callback', async () => {
      const client = makeFakeClient();
      const openFile = vi.fn();
      const ws = new VanylineWorkspace(client, openFile);
      const view = makeFakeView();
      ws.openFile('file:///src/main.rs', 'rust', view);

      const result = await ws.displayFile('file:///src/main.rs');

      expect(result).toBe(view);
      expect(openFile).not.toHaveBeenCalled();
    });

    it('fichier hors workspace (toolchains/...) : null, pas de callback', async () => {
      const client = makeFakeClient();
      const openFile = vi.fn();
      const ws = new VanylineWorkspace(client, openFile);

      const result = await ws.displayFile(
        'file:///toolchains/rust/usr/local/rustup/toolchains/x/lib/rustlib/src/rust/library/core/src/option.rs',
      );

      expect(result).toBeNull();
      expect(openFile).not.toHaveBeenCalled();
    });

    it('nouveau fichier workspace : appelle le callback, attend l\'enregistrement via openFile', async () => {
      const client = makeFakeClient();
      const view = makeFakeView();
      let ws!: VanylineWorkspace;
      const openFile = vi.fn((path: string) => {
        // Simule Editor.vue qui monte, charge le fichier, et s'enregistre —
        // asynchrone en pratique, ici volontairement différé d'un tick.
        setTimeout(() => ws.openFile(`file:///${path}`, 'rust', view), 10);
      });
      ws = new VanylineWorkspace(client, openFile);

      const result = await ws.displayFile('file:///src/lib.rs');

      expect(openFile).toHaveBeenCalledWith('src/lib.rs');
      expect(result).toBe(view);
    });

    it('sans callback fourni : null (pas de crash)', async () => {
      const client = makeFakeClient();
      const ws = new VanylineWorkspace(client);

      const result = await ws.displayFile('file:///src/lib.rs');

      expect(result).toBeNull();
    });
  });
});