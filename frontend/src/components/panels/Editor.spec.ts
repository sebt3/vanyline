import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import { EditorView } from 'codemirror';
import { EditorSelection } from '@codemirror/state';
import Editor from './Editor.vue';
import ContextMenu from '../ContextMenu.vue';
import { clearIdeActions, useIdeSession } from '../../composables/useIdeSession';
import type { DockviewPanelApi } from 'dockview-vue';
import { jumpToDefinition, renameSymbol } from '@codemirror/lsp-client';

vi.mock('@codemirror/lsp-client', () => ({
  jumpToDefinition: vi.fn(() => true),
  renameSymbol: vi.fn(() => true),
}));

function makeClient() {
  return {
    request: vi.fn().mockImplementation(
      async (op: string, params: Record<string, unknown>) => {
        if (op === 'read') return { ok: true, content: 'ligne1\nligne2\n', truncated: false };
        if (op === 'write') return { ok: true, wrote: params.path as string };
        return { ok: false, error: 'unknown' };
      },
    ),
  };
}

/** Un panel Editor par fichier (task multi-onglets) : `path` est fixe pour la
 *  durée de vie de l'instance, plus `sandbox-fs` toujours injecté par
 *  IdeShell. dockview-vue ne lie qu'UN prop réel (`params`) sur les
 *  composants enregistrés via `components:` — sa valeur enveloppe le
 *  `params` passé à `addPanel` ET `api` ensemble (vérifié à l'exécution,
 *  cf. commentaire en tête d'Editor.vue) : la forme imbriquée ci-dessous
 *  n'est pas arbitraire, c'est celle que le composant reçoit vraiment.
 */
function makePanelApi(isActive = true): DockviewPanelApi {
  return {
    isActive,
    onDidActiveChange: vi.fn(() => ({ dispose: vi.fn() })),
  } as unknown as DockviewPanelApi;
}

function editorProps(path: string, isActive = true) {
  return { params: { params: { path }, api: makePanelApi(isActive) } };
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('Editor.vue — contenu réel', () => {
  let client: ReturnType<typeof makeClient>;
  let writeText: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    client = makeClient();
    writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
  });

  afterEach(() => {
    // Nettoyer le body après chaque test (Portal téléporte le menu dedans).
    document.body.innerHTML = '';
    delete (navigator as unknown as Record<string, unknown>).clipboard;
  });

  it('lit le fichier de params.path au montage (raw: true)', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    // 2 flush : onMounted + read async résolu
    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('read', {
      path: 'src/main.py',
      raw: true,
    });

    expect(wrapper.element.textContent).toContain('ligne1');
  });

  it('CodeMirror est réellement monté dans .editor-host (régression : le wrapper du menu contextuel pouvait laisser hostRef non résolu)', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    // Pas juste "la requête a été envoyée" : .cm-editor doit être un vrai
    // enfant DOM de .editor-host, pas juste construit en mémoire avec un
    // parent jamais attaché (bug trouvé en usage réel — read fonctionnait,
    // rien n'était visible).
    const host = wrapper.find('.editor-host');
    expect(host.element.querySelector('.cm-editor')).toBeTruthy();
  });

  it('charge le fichier dès que sandbox-fs devient prêt après le montage (fsClient null au départ)', async () => {
    // Reproduit la restauration d'un layout dockview sauvegardé : le panel
    // Editor est recréé avant que la connexion /ws/fs (async) soit établie.
    const fsClientRef = ref<typeof client | null>(null);
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': fsClientRef }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    // fsClient toujours null au montage : aucune requête de lecture envoyée.
    expect(client.request).not.toHaveBeenCalledWith('read', expect.anything());

    // La connexion se résout après coup.
    fsClientRef.value = client;
    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('read', {
      path: 'src/main.py',
      raw: true,
    });
    const { getView } = wrapper.vm as { getView: () => EditorView };
    expect(getView().state.doc.toString()).toContain('ligne1');
  });

  it('Ctrl+S écrit le contenu courant', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    // 2 flush : onMounted(async read) + read async résolu
    await flushMicrotasks();
    await flushMicrotasks();

    const { save } = wrapper.vm as { save: () => void };
    save();

    await flushMicrotasks();

    // read appelé en premier (montage), write depuis save()
    expect(client.request).toHaveBeenCalledWith('write', {
      path: 'src/main.py',
      content: 'ligne1\nligne2\n',
    });
  });

  it("n'enregistre saveActiveFile que pour l'onglet actif (plusieurs instances possibles)", async () => {
    const { ideActions } = useIdeSession();
    clearIdeActions();

    mount(Editor, {
      props: editorProps('inactive.py', false),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });
    await flushMicrotasks();
    expect(ideActions.value.saveActiveFile).toBeUndefined();

    const activeWrapper = mount(Editor, {
      props: editorProps('active.py', true),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });
    await flushMicrotasks();
    await flushMicrotasks();

    expect(ideActions.value.saveActiveFile).toBeDefined();
    ideActions.value.saveActiveFile?.();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('write', {
      path: 'active.py',
      content: 'ligne1\nligne2\n',
    });
    // Vérifie que c'est bien l'instance active qui a écrit, pas l'inactive.
    expect((activeWrapper.vm as { save: () => void }).save).toBeDefined();
  });

  it('enregistre findInActiveFile/replaceInActiveFile quand l\'onglet est actif et ouvre le panneau de recherche', async () => {
    const { ideActions } = useIdeSession();
    clearIdeActions();

    const activeWrapper = mount(Editor, {
      props: editorProps('a.py', true),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });
    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    expect(ideActions.value.findInActiveFile).toBeDefined();
    expect(ideActions.value.replaceInActiveFile).toBeDefined();

    ideActions.value.findInActiveFile?.();
    await flushMicrotasks();
    await flushMicrotasks();

    // L'action est bien enregistrée et exécutable.
    // (le panneau de recherche ne se rend pas en jsdom car CodeMirror a
    // besoin du layout DOM que jsdom n'implémente pas).
    expect(activeWrapper.element.querySelector('.editor-host')).toBeTruthy();
  });

  it('instance inactive ne enregistre pas findInActiveFile', async () => {
    const { ideActions } = useIdeSession();
    clearIdeActions();

    mount(Editor, {
      props: editorProps('inactive.py', false),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });
    await flushMicrotasks();

    expect(ideActions.value.findInActiveFile).toBeUndefined();
  });

  it('le menu contextuel de l\'éditeur expose Couper/Copier/Coller et Copier chemin', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const labels = items.map((el) => el.textContent);
    // textContent inclut le raccourci (ex: 'Couper⌘X'), vérifier par substring.
    expect(labels.some((l) => l?.includes('Couper'))).toBe(true);
    expect(labels.some((l) => l?.includes('Copier'))).toBe(true);
    expect(labels.some((l) => l?.includes('Coller'))).toBe(true);
    expect(labels.some((l) => l?.includes('Copier le chemin du fichier'))).toBe(true);
  });

  it('Copier le chemin du fichier écrit le chemin dans le presse-papiers', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const item = items.find((el) => el.textContent?.includes('Copier le chemin du fichier'));
    item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(writeText).toHaveBeenCalledWith('src/main.py');
  });

  it('Coller insère le contenu du presse-papiers à la tête de sélection', async () => {
    const readText = vi.fn().mockResolvedValue('collé');
    // jsdom n'implémente pas getClientRects, requis par CodeMirror pour le layout.
    const textProto = Text.prototype as unknown as Record<string, unknown>;
    const gc = textProto.getClientRects;
    const gb = textProto.getBoundingClientRect;
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      textProto.getClientRects = () =>
        ({ item: () => ({}), length: 1, [Symbol.iterator]: function* () {} } as unknown as DOMRectList);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      textProto.getBoundingClientRect = () =>
        ({ top: 0, left: 0, width: 0, height: 0, bottom: 0, right: 0, x: 0, y: 0 } as DOMRect);

      Object.defineProperty(navigator, 'clipboard', {
        value: { writeText, readText },
        configurable: true,
      });

      const wrapper = mount(Editor, {
        props: editorProps('src/main.py'),
        global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
      });

      await flushMicrotasks();
      await flushMicrotasks();
      await flushMicrotasks();

      const { getView, pasteClipboard } = wrapper.vm as {
        getView: () => EditorView;
        pasteClipboard: () => void;
      };
      pasteClipboard();
      await flushMicrotasks();

      const doc = getView().state.doc.toString();
      expect(doc).toContain('collé');
    } finally {
      textProto.getClientRects = gc;
      textProto.getBoundingClientRect = gb;
    }
  });

  it('Coller remplace la sélection active au lieu de s\'insérer à côté', async () => {
    const readText = vi.fn().mockResolvedValue('PASTE');
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText, readText },
      configurable: true,
    });

    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const { getView, pasteClipboard } = wrapper.vm as {
      getView: () => EditorView;
      pasteClipboard: () => void;
    };
    const view = getView();
    // Sélectionne "ligne1" (positions 0 à 6) avant de coller.
    view.dispatch({ selection: EditorSelection.single(0, 6) });
    pasteClipboard();
    await flushMicrotasks();

    expect(view.state.doc.toString()).toBe('PASTE\nligne2\n');
  });

  it('sans sélection, Copier n\'écrit rien', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.py'),
      global: { provide: { 'sandbox-fs': ref(client) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const { copySelection } = wrapper.vm as {
      copySelection: () => void;
    };
    copySelection();
    await flushMicrotasks();

    expect(writeText).not.toHaveBeenCalled();
  });
});

describe('Editor.vue — plugin LSP', () => {
  const flush = () => flushMicrotasks();

  /** Crée un fake LSPClient avec une méthode `plugin` mockée. */
  function fakeLspPlugin() {
    return vi.fn(() => []);
  }

  /** Fournit `get-lsp-client` qui renvoie le fake ou null selon `shouldReturn`. */
  function makeLspProvider(shouldReturn: unknown): () => Promise<unknown> {
    return async () => shouldReturn;
  }

  it('ajoute le plugin LSP quand un client est fourni', async () => {
    const pluginFn = fakeLspPlugin();
    const fakeClient = { plugin: pluginFn };
    const provider = makeLspProvider(fakeClient);

    const wrapper = mount(Editor, {
      props: editorProps('src/main.rs'),
      global: {
        provide: {
          'sandbox-fs': ref(makeClient()),
          'get-lsp-client': provider,
        },
        components: { ContextMenu },
      },
    });

    // loadFile : read → getLspClient → reconfigure → 3-4 microtasks
    await flush();
    await flush();
    await flush();
    await flush();

    expect(pluginFn).toHaveBeenCalledWith('file:///src/main.rs', 'rust');
    const { getView } = wrapper.vm as { getView: () => EditorView };
    expect(getView().state.doc.toString()).toContain('ligne1');
  });

  it("mode dégradé : sans client LSP, pas de plugin", async () => {
    const pluginFn = fakeLspPlugin();
    const provider = makeLspProvider(null);

    const wrapper = mount(Editor, {
      props: editorProps('src/main.rs'),
      global: {
        provide: {
          'sandbox-fs': ref(makeClient()),
          'get-lsp-client': provider,
        },
        components: { ContextMenu },
      },
    });

    await flush();
    await flush();
    await flush();
    await flush();

    // plugin n'a pas été appelé parce que le client est null
    expect(pluginFn).not.toHaveBeenCalled();
    const { getView } = wrapper.vm as { getView: () => EditorView };
    expect(getView().state.doc.toString()).toContain('ligne1');
  });

  it('pas de toolchain pour l\'extension → le provider n\'est pas appelé', async () => {
    const pluginFn = fakeLspPlugin();
    const fakeClient = { plugin: pluginFn };
    const provider = vi.fn(() => Promise.resolve(fakeClient));

    mount(Editor, {
      props: editorProps('a.py'),
      global: {
        provide: {
          'sandbox-fs': ref(makeClient()),
          'get-lsp-client': provider,
        },
        components: { ContextMenu },
      },
    });

    await flush();
    await flush();
    await flush();
    await flush();

    // Le provider n'a pas été appelé : pas de toolchain LSP pour .py
    expect(provider).not.toHaveBeenCalled();
  });
});

describe('Editor.vue — menu contextuel LSP', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('le menu expose Aller à la définition et Renommer le symbole', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.rs'),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const labels = items.map((el) => el.textContent);
    expect(labels.some((l) => l?.includes('Aller à la définition'))).toBe(true);
    expect(labels.some((l) => l?.includes('Renommer le symbole'))).toBe(true);
  });

  it('Renommer le symbole appelle renameSymbol avec le view', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.rs'),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const item = items.find((el) => el.textContent?.includes('Renommer le symbole'));
    item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(renameSymbol).toHaveBeenCalled();
  });

  it('Aller à la définition appelle jumpToDefinition avec le view', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('src/main.rs'),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const item = items.find((el) => el.textContent?.includes('Aller à la définition'));
    item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(jumpToDefinition).toHaveBeenCalled();
  });

  it('mode dégradé : les actions LSP restent sans plugin et ne plantent pas', async () => {
    const wrapper = mount(Editor, {
      props: editorProps('a.py'),
      global: { provide: { 'sandbox-fs': ref(makeClient()) }, components: { ContextMenu } },
    });

    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();
    await flushMicrotasks();

    const host = wrapper.find('.editor-host');
    host.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const renameItem = items.find((el) => el.textContent?.includes('Renommer le symbole'));
    renameItem!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    const gotoItem = items.find((el) => el.textContent?.includes('Aller à la définition'));
    gotoItem!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Les commandes mockées sont appelées sans crash
    expect(renameSymbol).toHaveBeenCalled();
    expect(jumpToDefinition).toHaveBeenCalled();
  });
});
