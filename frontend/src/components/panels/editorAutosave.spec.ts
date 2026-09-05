import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { EditorView } from '@codemirror/view';
import { EditorState, StateEffect } from '@codemirror/state';
import type { Extension } from '@codemirror/state';
import {
  AUTOSAVE_DEBOUNCE_MS,
  DISK_RELOAD_EVENT,
  applyDiskReload,
  autosaveExtension,
  flushAllEditors,
  flushEditor,
  makeFileChangedHandler,
  makeFlushRequestHandler,
  registerEditorSync,
} from './editorAutosave';
import type { AutosaveFsClient, EditorAutosave } from './editorAutosave';

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

/** Client /ws/fs factice — le contrat attendu par l'extension est structurel
 *  (une seule méthode `request`) ; le cast évite de simulator le generic
 *  `<T>` de `SandboxFsClient.request` dans chaque test. */
function makeFsClient(
  impl?: (op: string, params: Record<string, unknown>) => Promise<unknown>,
): { client: AutosaveFsClient; request: ReturnType<typeof vi.fn> } {
  const request = vi.fn(
    impl ?? (async (_op: string, _params: Record<string, unknown>) => ({ ok: true })),
  );
  return { client: { request } as unknown as AutosaveFsClient, request };
}

function makeView(autosave: Extension, doc = ''): EditorView {
  return new EditorView({
    state: EditorState.create({ doc, extensions: [autosave] }),
  });
}

function typeText(view: EditorView, text: string): void {
  view.dispatch({
    changes: { from: view.state.doc.length, insert: text },
    userEvent: 'input',
  });
}

describe('editorAutosave — extension', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('autosave deboune à 300 ms, une écriture groupée par rafale', async () => {
    expect(AUTOSAVE_DEBOUNCE_MS).toBe(300);
    const { client, request } = makeFsClient();
    const onWriteSuccess = vi.fn();
    const onWriteError = vi.fn();
    const autosave = autosaveExtension({
      path: 'src/a.ts',
      getClient: () => client,
      onWriteSuccess,
      onWriteError,
    });
    const view = makeView(autosave);

    // Rafale t=0/10/20 : le timer part à la première frappe et n'est PAS
    // repoussé par les suivantes (pattern useSandboxState — sinon une frappe
    // continue ne serait jamais écrite).
    typeText(view, 'a');
    vi.advanceTimersByTime(10);
    typeText(view, 'b');
    vi.advanceTimersByTime(10);
    typeText(view, 'c');

    vi.advanceTimersByTime(279); // t = 299 : rien n'est encore écrit
    expect(request).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1); // t = 300 : UNE seule écriture, contenu final
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('write', { path: 'src/a.ts', content: 'abc' });
    await flushMicrotasks();
    expect(onWriteSuccess).toHaveBeenCalledTimes(1);
    expect(onWriteError).not.toHaveBeenCalled();
    view.destroy();
  });

  it('flush immédiat vide l’attente', async () => {
    const { client, request } = makeFsClient();
    const onWriteSuccess = vi.fn();
    const autosave = autosaveExtension({
      path: 'src/b.ts',
      getClient: () => client,
      onWriteSuccess,
      onWriteError: () => {},
    });
    const view = makeView(autosave);

    typeText(view, 'z');
    // hasPending (08b) : le drapeau interne est exposé — true dès qu'une
    // écriture autosave est en attente (debounce pas expiré).
    expect(autosave.hasPending()).toBe(true);
    expect(autosave.flush()).toBe(true);
    expect(autosave.hasPending()).toBe(false);
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('write', { path: 'src/b.ts', content: 'z' });

    // Rien après expiration du debounce : pas de doublon.
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS + 100);
    expect(request).toHaveBeenCalledTimes(1);

    // Plus rien en attente → flush() est un no-op.
    expect(autosave.flush()).toBe(false);
    expect(request).toHaveBeenCalledTimes(1);
    await flushMicrotasks();
    expect(onWriteSuccess).toHaveBeenCalledTimes(1);
    view.destroy();
  });

  it('transaction disk-reload n’écrit jamais', async () => {
    const { client, request } = makeFsClient();
    const onWriteError = vi.fn();
    const autosave = autosaveExtension({
      path: 'src/c.ts',
      getClient: () => client,
      onWriteSuccess: () => {},
      onWriteError,
    });
    const view = makeView(autosave, 'sur disque');

    applyDiskReload(view, 'x');
    expect(view.state.doc.toString()).toBe('x');

    // Au-delà du debounce : le reload a bien modifié le document (docChanged),
    // mais aucune écriture ne part — écrire du contenu venant du disque serait
    // la boucle disque→write→disque.
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS + 200);
    await flushMicrotasks();
    expect(request).not.toHaveBeenCalled();
    expect(onWriteError).not.toHaveBeenCalled();

    // Un userEvent manuel portant l'événement est ignoré de la même façon…
    view.dispatch({
      changes: { from: view.state.doc.length, insert: 'y' },
      userEvent: DISK_RELOAD_EVENT,
    });
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS + 200);
    expect(request).not.toHaveBeenCalled();

    // … y compris noyé dans une rafale MIXTE : un seul update contenant à la
    // fois une transaction de reload et une frappe → conservateur, rien.
    view.dispatch(
      { changes: { from: view.state.doc.length, insert: 'm' }, userEvent: DISK_RELOAD_EVENT },
      { changes: { from: view.state.doc.length, insert: 'q' }, userEvent: 'input' },
    );
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS + 200);
    expect(request).not.toHaveBeenCalled();

    // Une frappe franche (update sans reload) reprend le contrôle.
    typeText(view, 'r');
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS);
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('write', { path: 'src/c.ts', content: 'xymqr' });
    view.destroy();
  });

  it('registre de flush', async () => {
    const { client, request } = makeFsClient();
    const autosaveA = autosaveExtension({
      path: 'a.txt',
      getClient: () => client,
      onWriteSuccess: () => {},
      onWriteError: () => {},
    });
    const autosaveB = autosaveExtension({
      path: 'b.txt',
      getClient: () => client,
      onWriteSuccess: () => {},
      onWriteError: () => {},
    });
    const viewA = makeView(autosaveA);
    const viewB = makeView(autosaveB);

    // Registre évolué 08b : registerEditorSync (flush/reload/hasPending) —
    // le flush de l'extension est un EditorSyncHooks.flush valide, même sens
    // qu'avant (le reload/hasPending factices ne sont pas exercés ici).
    const unregisterA = registerEditorSync('a.txt', {
      flush: autosaveA.flush,
      reload: () => {},
      hasPending: () => false,
    });
    const unregisterB = registerEditorSync('b.txt', {
      flush: autosaveB.flush,
      reload: () => {},
      hasPending: () => false,
    });

    typeText(viewA, 'AAA');
    typeText(viewB, 'BBB');
    flushAllEditors();
    expect(request).toHaveBeenCalledTimes(2);
    expect(request).toHaveBeenCalledWith('write', { path: 'a.txt', content: 'AAA' });
    expect(request).toHaveBeenCalledWith('write', { path: 'b.txt', content: 'BBB' });

    unregisterA();
    unregisterB();
    typeText(viewA, 'CCC');
    flushAllEditors();
    await flushMicrotasks();
    // Plus aucun enregistrement : le nouvel edit en attente n'est pas écrit
    // par le registre (les timers propres à l'extension restent seuls maîtres).
    expect(request).toHaveBeenCalledTimes(2);
    viewA.destroy();
    viewB.destroy();
  });

  it('client null abandonne sans erreur', async () => {
    const onWriteSuccess = vi.fn();
    const onWriteError = vi.fn();
    const autosave = autosaveExtension({
      path: 'src/d.ts',
      getClient: () => null,
      onWriteSuccess,
      onWriteError,
    });
    const view = makeView(autosave);

    typeText(view, 'a');
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS);
    await flushMicrotasks();
    expect(onWriteError).not.toHaveBeenCalled();
    expect(onWriteSuccess).not.toHaveBeenCalled();

    // Idem côté flush explicite (Ctrl+S / démontage) : abandon silencieux,
    // pas de throw — la prochaine frappe réessaiera. (Le timer du dessus a
    // déjà consommé la rafale : on en réarme une nouvelle.)
    typeText(view, 'b');
    expect(autosave.flush()).toBe(true);
    await flushMicrotasks();
    expect(onWriteError).not.toHaveBeenCalled();
    view.destroy();
  });

  it('échec du write remonte onWriteError', async () => {
    const { client, request } = makeFsClient(async () => {
      throw new Error('disque plein');
    });
    const onWriteSuccess = vi.fn();
    const onWriteError = vi.fn();
    const autosave = autosaveExtension({
      path: 'src/e.ts',
      getClient: () => client,
      onWriteSuccess,
      onWriteError,
    });
    const view = makeView(autosave);

    typeText(view, 'a');
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS);
    await flushMicrotasks();
    expect(request).toHaveBeenCalledTimes(1);
    expect(onWriteError).toHaveBeenCalledWith('disque plein');
    expect(onWriteSuccess).not.toHaveBeenCalled();
    // Rejet attrapé (pas de rejet non géré : vitest ferait échouer le test).
    view.destroy();
  });

  it('reconfigure LSP : la même instance autosave dupliquée dans la config n’est injectée qu’une fois', async () => {
    // Piège de la reconfigure LSP d'Editor.vue : baseExtensions (déjà dans la
    // config initiale) est spreadée dans StateEffect.reconfigure — si
    // l'extension était double-injectée, deux updateListener écriraient par
    // frappe. La config est reconstruite entière → instance unique ; et CM6
    // dédupplique par identité (flatten `seen`) — vérifié ici en listing
    // explicitement l'instance deux fois.
    const { client, request } = makeFsClient();
    const autosave: EditorAutosave = autosaveExtension({
      path: 'src/f.ts',
      getClient: () => client,
      onWriteSuccess: () => {},
      onWriteError: () => {},
    });
    const view = makeView(autosave, 'base');
    const fakePlugin = EditorView.updateListener.of(() => {});

    view.dispatch({
      effects: StateEffect.reconfigure.of([[autosave], [autosave, fakePlugin]]),
    });
    expect(view.state.doc.toString()).toBe('base');

    typeText(view, 'x');
    vi.advanceTimersByTime(AUTOSAVE_DEBOUNCE_MS);
    await flushMicrotasks();
    expect(request).toHaveBeenCalledTimes(1);
    view.destroy();
  });
});

describe('editorAutosave — registre', () => {
  it('dernier gagne par path et unregister gardé par identité', () => {
    const first = vi.fn();
    const second = vi.fn();
    const unregisterFirst = registerEditorSync('same.txt', {
      flush: first,
      reload: () => {},
      hasPending: () => false,
    });

    registerEditorSync('same.txt', { flush: second, reload: () => {}, hasPending: () => false });
    unregisterFirst(); // l'instance remplacée ne doit PAS débrancher la nouvelle
    flushAllEditors();
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();
  });
});

describe('editorAutosave — handler file-changed (08b)', () => {
  it('makeFileChangedHandler recharge via read raw', async () => {
    const reload = vi.fn();
    const unregister = registerEditorSync('a.rs', {
      flush: () => false,
      reload,
      hasPending: () => false,
    });
    const { client, request } = makeFsClient(async () => ({
      ok: true,
      content: 'contenu disque',
    }));
    const handler = makeFileChangedHandler(() => client);

    handler({ path: 'a.rs' });
    await flushMicrotasks();

    // Read OBLIGATOIREMENT raw:true — sans ça le serveur rend un contenu
    // numéroté ("    1\t…") qui corromprait le buffer (pattern loadFile).
    expect(request).toHaveBeenCalledWith('read', { path: 'a.rs', raw: true });
    expect(reload).toHaveBeenCalledWith('contenu disque');

    // Pas d'éditeur enregistré pour ce path → aucun read émis.
    request.mockClear();
    reload.mockClear();
    handler({ path: 'z.rs' });
    await flushMicrotasks();
    expect(request).not.toHaveBeenCalled();
    expect(reload).not.toHaveBeenCalled();

    // path absent/non-string → rien (frame malformée tolérée).
    handler({});
    handler({ path: 42 });
    await flushMicrotasks();
    expect(request).not.toHaveBeenCalled();

    unregister();
  });

  it('hasPending skippe le reload (frappe en cours, last-writer-wins assumé)', async () => {
    const reload = vi.fn();
    const unregister = registerEditorSync('b.rs', {
      flush: () => false,
      reload,
      hasPending: () => true,
    });
    const { client, request } = makeFsClient(async () => ({ ok: true, content: 'x' }));
    const handler = makeFileChangedHandler(() => client);

    handler({ path: 'b.rs' });
    await flushMicrotasks();
    expect(request).not.toHaveBeenCalled();
    expect(reload).not.toHaveBeenCalled();
    unregister();
  });

  it('read échoué ou client absent : le buffer reste, rien ne plante', async () => {
    const reload = vi.fn();
    const unregister = registerEditorSync('c.rs', {
      flush: () => false,
      reload,
      hasPending: () => false,
    });

    // Échec du read (rejet, comme SandboxFsClient sur ok:false) → pas de reload.
    const { client } = makeFsClient(async () => {
      throw new Error('fichier disparu');
    });
    makeFileChangedHandler(() => client)({ path: 'c.rs' });
    await flushMicrotasks();
    expect(reload).not.toHaveBeenCalled();

    // Client null (WS pas encore connecté) : abandon silencieux.
    makeFileChangedHandler(() => null)({ path: 'c.rs' });
    await flushMicrotasks();
    expect(reload).not.toHaveBeenCalled();

    unregister();
  });
});

describe('editorAutosave — handler flush-request (08c)', () => {
  it('flushEditor : false sans onglet, propage le hook flush sinon', () => {
    // Aucun onglet sur ce path → false (cas « rien à flush » vu du registre).
    expect(flushEditor('inconnu.rs')).toBe(false);

    const flush = vi.fn(() => true);
    const unregister = registerEditorSync('e.rs', {
      flush,
      reload: () => {},
      hasPending: () => true, // hasPending sans rapport : flushEditor délègue
    });
    expect(flushEditor('e.rs')).toBe(true);
    expect(flush).toHaveBeenCalledTimes(1);
    unregister();
    expect(flushEditor('e.rs')).toBe(false);
  });

  it('flush-request fait flush puis ack via la queue', async () => {
    // Liste d'appels partagée : l'ORDRE flush → request est l'assertion clé —
    // l'ack doit partir APRÈS le write enfilé par flush() (FIFO de la queue
    // client ; un ws.send brut passerait devant l'écriture).
    const calls: string[] = [];
    const flush = vi.fn(() => {
      calls.push('flush');
      return true;
    });
    const unregister = registerEditorSync('a.rs', {
      flush,
      reload: () => {},
      hasPending: () => false,
    });
    const { client, request } = makeFsClient(async () => {
      calls.push('request');
      return { ok: true };
    });
    const handler = makeFlushRequestHandler(() => client);

    handler({ id: 7, path: 'a.rs' });
    await flushMicrotasks();

    expect(flush).toHaveBeenCalledTimes(1);
    // Ack via request, avec `ackFor` SÉPARÉ de l'id de corrélation : un params
    // `id` écraserait celui que SandboxFsClient pose lui-même (pending jamais
    // résolu → queue FIFO bloquée à vie — piège vérifié, asserti en dessous).
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('flush-ack', { ackFor: 7 });
    expect(request.mock.calls[0][1]).not.toHaveProperty('id');
    expect(calls).toEqual(['flush', 'request']);
    unregister();
  });

  it('chemin inconnu acknowledge quand même', async () => {
    // Aucun onglet sur z.rs : « rien à flush » est un SUCCÈS — le serveur
    // résout au premier ack et n'a pas besoin de savoir qui tient le fichier
    // (le broadcast touche toutes les sessions, chacune acquitte).
    const { client, request } = makeFsClient();
    const handler = makeFlushRequestHandler(() => client);

    handler({ id: 8, path: 'z.rs' });
    await flushMicrotasks();

    expect(request).toHaveBeenCalledWith('flush-ack', { ackFor: 8 });
  });

  it('event sans id numérique ignoré', async () => {
    const flush = vi.fn(() => true);
    const unregister = registerEditorSync('a.rs', {
      flush,
      reload: () => {},
      hasPending: () => false,
    });
    const { client, request } = makeFsClient();
    const handler = makeFlushRequestHandler(() => client);

    handler({ path: 'a.rs' }); // id absent
    handler({ id: '7', path: 'a.rs' }); // id non numérique
    await flushMicrotasks();

    // Sans id numérique, aucun ackFor renvoyable : le serveur n'aurait rien à
    // résoudre — frame ignorée, et pas de flush non plus (aucune requête
    // légitime derrière).
    expect(flush).not.toHaveBeenCalled();
    expect(request).not.toHaveBeenCalled();
    unregister();
  });

  it('client null ou ack en échec : fire-and-forget, rien ne remonte', async () => {
    // WS fermé en vol → request rejeté : avalé (de toute façon le serveur
    // retombe sur son timeout). Aucun rejet non géré ne doit fuite (vitest).
    const { client } = makeFsClient(async () => {
      throw new Error('ws fermé');
    });
    const unregister = registerEditorSync('f.rs', {
      flush: () => true,
      reload: () => {},
      hasPending: () => false,
    });
    makeFlushRequestHandler(() => client)({ id: 9, path: 'f.rs' });
    makeFlushRequestHandler(() => null)({ id: 9, path: 'f.rs' });
    await flushMicrotasks();
    unregister();
  });
});
