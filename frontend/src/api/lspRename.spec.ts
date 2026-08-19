import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { Text, EditorState } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import type { LSPClient } from '@codemirror/lsp-client';
import { LSPPlugin } from '@codemirror/lsp-client';
import {
  positionToOffset,
  offsetToPosition,
  applyTextEditsToString,
  workspaceEditFiles,
  renameSymbolCustom,
  renameSymbolFromView,
  type TextEdit,
  type WorkspaceEdit,
} from './lspRename';
import type { SandboxFsClient } from './sandboxWs';

// Helpers de construction de mocks

function makeLspClient() {
  const sync = vi.fn();
  const request = vi.fn();
  const getFile = vi.fn();
  return {
    sync,
    request,
    getFile,
    client: { sync, request, workspace: { getFile } },
  };
}

function makeFsClient(): SandboxFsClient & { request: ReturnType<typeof vi.fn> } {
  const request = vi.fn().mockImplementation(async (op: string) => {
    if (op === 'read') return { ok: true, content: 'abc' };
    if (op === 'write') return { ok: true };
    return { ok: false, error: 'unknown' };
  });
  return Object.assign(
    {} as SandboxFsClient,
    { request },
  ) as SandboxFsClient & { request: ReturnType<typeof vi.fn> };
}

// positionToOffset

describe('lspRename — positionToOffset', () => {
  it('convertit et clampe', () => {
    const doc = Text.of(['abc', 'def']);
    expect(positionToOffset(doc, { line: 1, character: 2 })).toBe(6);
    expect(positionToOffset(doc, { line: 0, character: 99 })).toBe(3);
    expect(positionToOffset(doc, { line: 99, character: 0 })).toBe(doc.length);
  });
});

// offsetToPosition

describe('lspRename — offsetToPosition', () => {
  it('convertit', () => {
    expect(offsetToPosition(Text.of(['abc', 'def']), 6)).toEqual(
      { line: 1, character: 2 },
    );
  });
});

// applyTextEditsToString

describe('lspRename — applyTextEditsToString', () => {
  it('applique et trie décroissant', () => {
    const result = applyTextEditsToString('hello world', [
      { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } }, newText: 'HELLO' },
    ] as TextEdit[]);
    expect(result).toBe('HELLO world');
  });
});

// workspaceEditFiles

describe('lspRename — workspaceEditFiles', () => {
  it('combine changes et documentChanges', () => {
    const e1 = { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: 'a' } as TextEdit;
    const e2 = { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: 'b' } as TextEdit;
    const e3 = { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: 'c' } as TextEdit;
    const edit: WorkspaceEdit = {
      changes: {
        'file:///a.rs': [e1],
        'file:///b.rs': [e2],
      },
      documentChanges: [{ textDocument: { uri: 'file:///c.rs' }, edits: [e3] }],
    };
    const result = workspaceEditFiles(edit);
    expect(result).toHaveLength(3);
    expect(result[0].uri).toBe('file:///a.rs');
    expect(result[1].uri).toBe('file:///b.rs');
    expect(result[2].uri).toBe('file:///c.rs');
  });

  it('concatène edits de même URI dans changes et documentChanges', () => {
    const e1 = { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: 'a' } as TextEdit;
    const e2 = { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: 'b' } as TextEdit;
    const edit: WorkspaceEdit = {
      changes: { 'file:///x.rs': [e1] },
      documentChanges: [{ textDocument: { uri: 'file:///x.rs' }, edits: [e2] }],
    };
    const result = workspaceEditFiles(edit);
    expect(result).toHaveLength(1);
    expect(result[0].edits).toHaveLength(2);
  });

  it('ignore les edits vides', () => {
    const edit: WorkspaceEdit = {
      changes: { 'file:///x.rs': [] },
    };
    const result = workspaceEditFiles(edit);
    expect(result).toHaveLength(0);
  });
});

// renameSymbolCustom

describe('lspRename — renameSymbolCustom', () => {
  it('applique fichier ouvert via dispatch', async () => {
    const { sync, request, getFile } = makeLspClient();
    const client = { sync, request, workspace: { getFile } } as unknown as LSPClient;
    const fs = makeFsClient();
    const doc = Text.of(['abc']);
    const view = {
      state: { doc },
      dispatch: vi.fn(),
      wordAt: vi.fn(),
    } as unknown as EditorView;

    getFile.mockReturnValue({ getView: () => view });
    request.mockResolvedValue({
      changes: {
        'file:///src/main.rs': [
          { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, newText: 'FOO' },
        ],
      },
    } as WorkspaceEdit);

    const result = await renameSymbolCustom(
      client,
      view,
      'file:///src/main.rs',
      0,
      fs,
      'FOO',
    );

    expect(sync).toHaveBeenCalledBefore(request);
    expect(request).toHaveBeenCalledWith('textDocument/rename', {
      textDocument: { uri: 'file:///src/main.rs' },
      position: { line: 0, character: 0 },
      newName: 'FOO',
    });
    expect(view.dispatch).toHaveBeenCalledWith({
      changes: [{ from: 0, to: 3, insert: 'FOO' }],
      userEvent: 'rename',
    });
    expect(result).toEqual({ applied: ['src/main.rs'], failed: [] });
  });

  it('applique fichier non ouvert via fs read/write', async () => {
    const { request, getFile, sync } = makeLspClient();
    const client = { sync, request, workspace: { getFile } } as unknown as LSPClient;
    const fs = makeFsClient();
    const view = {
      state: { doc: Text.of(['abc']) },
      dispatch: vi.fn(),
      wordAt: vi.fn(),
    } as unknown as EditorView;

    getFile.mockReturnValue(null);
    request.mockResolvedValue({
      changes: {
        'file:///lib/other.rs': [
          { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, newText: 'FOO' },
        ],
      },
    } as WorkspaceEdit);

    const result = await renameSymbolCustom(
      client as unknown as LSPClient,
      view,
      'file:///src/main.rs',
      0,
      fs,
      'BAR',
    );

    expect(fs.request).toHaveBeenCalledWith('read', { path: 'lib/other.rs', raw: true });
    expect(fs.request).toHaveBeenCalledWith('write', { path: 'lib/other.rs', content: 'FOO' });
    expect(result).toEqual({ applied: ['lib/other.rs'], failed: [] });
  });

  it('pas de rename si result null', async () => {
    const { request, sync } = makeLspClient();
    const client = { sync, request } as unknown as LSPClient;
    const fs = makeFsClient();
    const view = {
      state: { doc: Text.of(['abc']) },
      dispatch: vi.fn(),
    } as unknown as EditorView;

    request.mockResolvedValue(null as unknown as WorkspaceEdit);

    const result = await renameSymbolCustom(
      client,
      view,
      'file:///src/main.rs',
      0,
      fs,
      'FOO',
    );

    expect(result).toEqual({ applied: [], failed: [] });
    expect(view.dispatch).not.toHaveBeenCalled();
    expect(fs.request).not.toHaveBeenCalled();
  });

  it('throw si requête échoue', async () => {
    const { request, sync } = makeLspClient();
    const client = { sync, request } as unknown as LSPClient;
    const fs = makeFsClient();
    const view = {
      state: { doc: Text.of(['abc']) },
      dispatch: vi.fn(),
    } as unknown as EditorView;

    request.mockRejectedValue(new Error('LSP error'));

    await expect(
      renameSymbolCustom(
        client,
        view,
        'file:///src/main.rs',
        0,
        fs,
        'FOO',
      ),
    ).rejects.toThrow('LSP error');
  });

  it('best effort : continue sur échec', async () => {
    const { request, getFile, sync } = makeLspClient();
    const client = { sync, request, workspace: { getFile } } as unknown as LSPClient;
    const fs = makeFsClient();
    const openView = {
      state: { doc: Text.of(['abc']) },
      dispatch: vi.fn(),
    } as unknown as EditorView;

    getFile.mockImplementation((uri: string) => {
      if (uri === 'file:///src/main.rs') return { getView: () => openView };
      return null;
    });

    fs.request.mockImplementation(async (op: string) => {
      if (op === 'read') return { ok: true, content: 'abc' };
      if (op === 'write') throw new Error('write failed');
      return { ok: false };
    });

    request.mockResolvedValue({
      changes: {
        'file:///src/main.rs': [
          { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, newText: 'FOO' },
        ],
        'file:///x.rs': [
          { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, newText: 'FOO' },
        ],
      },
    } as WorkspaceEdit);

    const result = await renameSymbolCustom(
      client as unknown as LSPClient,
      openView,
      'file:///src/main.rs',
      0,
      fs,
      'FOO',
    );

    expect(result.applied).toContain('src/main.rs');
    expect(result.failed).toContain('x.rs');
    expect(result.failed.length).toBe(1);
  });
});

// renameSymbolFromView

describe('lspRename — renameSymbolFromView', () => {
  let spyPlugin: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    spyPlugin = vi.spyOn(LSPPlugin, 'get');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('demande nom et renomme', async () => {
    const { request, getFile, sync } = makeLspClient();
    const fs = makeFsClient();
    const spyPrompt = vi.spyOn(window, 'prompt').mockReturnValue('FOO');

    const realState = EditorState.create({ doc: 'foobar' });
    const view = {
      state: realState,
      dispatch: vi.fn(),
    } as unknown as EditorView;

    const fakePlugin = {
      client: { sync, request, workspace: { getFile } },
      uri: 'file:///src/main.rs',
    } as unknown as LSPClient;
    spyPlugin.mockReturnValue(fakePlugin);
    getFile.mockReturnValue({ getView: () => ({ state: { doc: Text.of(['abc']) } }) });
    request.mockResolvedValue({
      changes: {
        'file:///src/main.rs': [
          { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, newText: 'FOO' },
        ],
      },
    } as WorkspaceEdit);

    const result = await renameSymbolFromView(view, fs);

    expect(result).toContain('Renommé');
    expect(request).toHaveBeenCalledWith('textDocument/rename', {
      textDocument: { uri: 'file:///src/main.rs' },
      position: { line: 0, character: 0 },
      newName: 'FOO',
    });
    spyPrompt.mockRestore();
  });

  it('annulé rend vide', async () => {
    const { request, sync } = makeLspClient();
    const fs = makeFsClient();
    const spyPrompt = vi.spyOn(window, 'prompt').mockReturnValue(null);

    const realState = EditorState.create({ doc: 'foobar' });
    const view = {
      state: realState,
      dispatch: vi.fn(),
    } as unknown as EditorView;

    spyPlugin.mockReturnValue({
      client: { sync, request },
      uri: 'file:///src/main.rs',
    } as unknown as LSPClient);

    const result = await renameSymbolFromView(view, fs);

    expect(result).toBe('');
    expect(request).not.toHaveBeenCalled();
    spyPrompt.mockRestore();
  });

  it('sans plugin rend vide', async () => {
    const fs = makeFsClient();
    spyPlugin.mockReturnValue(null);

    const realState = EditorState.create({ doc: 'foobar' });
    const view = {
      state: realState,
      dispatch: vi.fn(),
    } as unknown as EditorView;

    const result = await renameSymbolFromView(view, fs);

    expect(result).toBe('');
  });
});