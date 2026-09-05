import { EditorView } from '@codemirror/view';
import { Transaction } from '@codemirror/state';
import type { Extension } from '@codemirror/state';

/**
 * Autosave éditeur (feature lsp-agent-interface, tâches 07a + 08b + 08c).
 *
 * Une extension CodeMirror par onglet : document modifié → `write` debouncé
 * vers /ws/fs, exactement la payload du save manuel (`Editor.vue`), sérialisée
 * par la queue du `SandboxFsClient` (le serveur est strictement
 * requête→réponse : écrire hors de cette queue corromprait le protocole).
 *
 * La tâche 08b pose le volet « fichier changé sur disque » : le client
 * `SandboxFsClient` route désormais les frames push (`onEvent`), ce module
 * fournit le registre path→hooks de synchronisation (flush/reload/hasPending)
 * et `makeFileChangedHandler` — lecture raw puis rechargement du buffer via
 * la transaction `DISK_RELOAD_EVENT`. L'émetteur d'événements sera
 * `edit_and_check` (08d) ; le déclencheur n'est donc pas encore branché.
 *
 * La tâche 08c pose l'aller-retour « flush avant écriture » (cas B R1 sq3) :
 * `flushEditor` (flush d'un onglet par path) et `makeFlushRequestHandler` —
 * réception du push `flush-request`, flush local, ack via la queue FIFO avec
 * le champ dédié `ackFor` (jamais `id` en params, jamais `ws.send` brut).
 */

/** Événement utilisateur posé sur TOUTE transaction de rechargement depuis le
 *  disque ; l'autosave ne déclenche JAMAIS d'écriture sur une transaction
 *  portant cet événement (le contenu vient du disque, l'écrire serait une
 *  boucle). Précédent des userEvent custom : 'rename' (lspRename.ts). */
export const DISK_RELOAD_EVENT = 'disk-reload';

/** Debounce retenu (fourchette design R1 sous-question 3 : 200-500 ms). */
export const AUTOSAVE_DEBOUNCE_MS = 300;

/** Le strict minimum du client /ws/fs attendu : la méthode `request`, dont la
 *  queue interne sérialise les écritures (contrainte protocole, cf.
 *  SandboxFsClient). Forme structurelle identique à celle du prototype. */
export interface AutosaveFsClient {
  request: <T>(op: string, params: Record<string, unknown>) => Promise<T>;
}

/** Valeur renvoyée par `autosaveExtension` : UN EXTENSION VALIDE (le contrat
 *  `: Extension` est tenu — CM6 aplatit tout objet `{ extension }` et ignore
 *  les propriétés supplémentaires, vérifié dans `flatten()` de
 *  @codemirror/state) portant en plus le `flush` de cette instance. Le
 *  composant doit en effet pouvoir flusher CETTE instance (Ctrl+S, démontage,
 *  defineExpose) et rien d'autre dans le prototype ne le permet ; `flush`
 *  renvoie `true` si une écriture en attente a été exécutée (sauve la
 *  double écriture de save(), cf. Editor.vue). `hasPending` (08b) expose le
 *  drapeau interne : y a-t-il une écriture autosave en attente ? */
export type EditorAutosave = Extension & {
  flush: () => boolean;
  hasPending: () => boolean;
};

/** Hooks de synchronisation d'un onglet éditeur (registre 08b) : ce que le
 *  handler `file-changed` et le flush global (08/08c) savent faire d'un
 *  buffer ouvert. */
export interface EditorSyncHooks {
  /** Vide l'écriture autosave en attente ; `true` si une écriture a été
   *  consommée. */
  flush: () => boolean;
  /** Recharge le buffer depuis un contenu disque (transaction DISK_RELOAD_EVENT). */
  reload: (content: string) => void;
  /** Une écriture autosave est-elle en attente (debounce pas expiré) ? */
  hasPending: () => boolean;
}

// Registre module path→hooks de sync : un seul enregistrement par path
// (dernier gagne) — un onglet = un path = une vue.
const syncRegistry = new Map<string, EditorSyncHooks>();

/** Enregistre les hooks de synchronisation d'un onglet par path ; renvoie la
 *  fonction de désenregistrement (appelée au démontage de l'instance).
 *  Remplace `registerEditorFlush` (07a), évolution propre — 07a non mergé.
 *
 *  Le désenregistrement est gardé par identité : si un nouvel enregistrement
 *  a remplacé celui-ci (même path, instance recréée avant démontage de
 *  l'ancienne), l'unregister tardif de l'ancienne ne doit pas la débrancher. */
export function registerEditorSync(path: string, hooks: EditorSyncHooks): () => void {
  syncRegistry.set(path, hooks);
  return () => {
    if (syncRegistry.get(path) === hooks) syncRegistry.delete(path);
  };
}

/** Vide le registre : notifie le flush de TOUS les éditeurs enregistrés.
 *  Utilisé par la tâche 08 (flush global avant édition LLM) ; signature
 *  conservée de 07a. Copie itérative : un flush qui se désenregistre lui-même
 *  ne doit pas fausser la boucle. */
export function flushAllEditors(): void {
  for (const hooks of [...syncRegistry.values()]) hooks.flush();
}

/** Flush d'UN onglet par path ; false si aucun onglet ou rien d'en attente
 *  (08c : cible du handler `flush-request` — le path réclamé par le serveur ;
 *  multi-onglets, chaque session WS appelante acquittera de toute façon, le
 *  serveur résout au premier ack). */
export function flushEditor(path: string): boolean {
  const hooks = syncRegistry.get(path);
  if (!hooks) return false;
  return hooks.flush();
}

/** Recharge le buffer d'une vue depuis un contenu venant du disque :
 *  transaction full-doc, `userEvent: DISK_RELOAD_EVENT`. Ne fait AUCUNE
 *  écriture, AUCUNE lecture. Appelé par le hook `reload` de l'onglet, lui-même
 *  ciblé par `makeFileChangedHandler` (canal push câblé en 08b). */
export function applyDiskReload(view: EditorView, content: string): void {
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: content },
    userEvent: DISK_RELOAD_EVENT,
  });
}

/** Handler de l'événement `file-changed` (frame push /ws/fs, tâche 08b) :
 *  si un éditeur tient `path` → skip si une écriture autosave est en cours
 *  (`hasPending`) ; sinon `read {path, raw:true}` via le client fourni puis
 *  `reload(content)`. Read échoué → RIEN : le buffer reste tel quel (un
 *  reload raté ne doit pas vider l'éditeur — l'événement suivant réessaiera).
 *
 *  Skip `hasPending` = LAST-WRITER-WINS ASSUMÉ : une frappe humaine dont le
 *  debounce (300 ms) n'a pas expiré gagne — le flush partira APRÈS le reload
 *  et réécrira le disque par-dessus l'édition LLM. L'anti-course strict
 *  (flush de la cible avant toute écriture LLM) est le « flush cas B » de la
 *  tâche 08c, pas d'ici.
 *
 *  Echo du sien : à ce jour un `write` via /ws/fs n'émet PAS d'événement
 *  (l'émetteur unique sera edit_and_check en 08d) — pas d'auto-echo à
 *  filtrer. Si un futur émetteur déclenchait sur les writes aussi, deux
 *  gardes protègent la frappe : `hasPending` ci-dessus (fenêtre du debounce)
 *  et le marqueur `DISK_RELOAD_EVENT` porté par `reload` — l'autosave
 *  n'écrit JAMAIS une transaction marquée, donc un contenu qui vient du
 *  disque ne repart jamais en écriture (anti-boucle). NOTE vérifiée : CM6 ne
 *  réduit PAS en no-op une transaction plein-document sur contenu identique
 *  (`docChanged` reste true) — ce n'est pas un garde sur lequel compter. */
export function makeFileChangedHandler(
  getClient: () => AutosaveFsClient | null,
): (event: { path?: unknown }) => void {
  return (event: { path?: unknown }) => {
    const path = typeof event.path === 'string' ? event.path : undefined;
    if (!path) return; // frame sans path exploitable : rien à faire
    const hooks = syncRegistry.get(path);
    if (!hooks) return; // aucun éditeur ouvert sur ce path : rien à recharger
    if (hooks.hasPending()) return; // frappe en cours — cf. last-writer-wins ci-dessus
    const client = getClient();
    if (!client) return; // WS pas connecté : rien à lire, événement perdu assumé
    // Read en mode RAW obligatoire : sans `raw:true` le serveur rend un
    // contenu NUMÉROTÉ («    1\t… ») qui corromprait le buffer (même
    // exigence que loadFile dans Editor.vue).
    client
      .request<{ ok: boolean; content: string }>('read', { path, raw: true })
      .then(
        (resp) => hooks.reload(resp.content),
        // ok:false est converti en rejet par SandboxFsClient ; tout échec
        // (réseau, fichier disparu) laisse le buffer inchangé, sans statut.
        () => {},
      );
  };
}

/** Handler de l'événement `{"event":"flush-request","id":N,"path":…}` (08c,
 *  arbitrage R1 sq3 cas B) : le serveur (edit_and_check en 08d) réclame un
 *  flush AVANT d'écrire et attend l'ack. `flushEditor(path)` d'abord — chemin
 *  inconnu ou rien d'en attente = « rien à flush », un SUCCÈS : on ack
 *  QUAND MÊME (le broadcast touche toutes les sessions, chacune acquitte,
 *  le serveur résout au premier ack et n'a pas besoin de qui tient le
 *  fichier).
 *
 *  ACK via `client.request('flush-ack', { ackFor: id })` :
 *  - JAMAIS `ws.send` brut : `flush()` enfile son `write` dans la queue FIFO
 *    du client ; un send brut partirait devant et l'ack précéderait l'écriture
 *    — l'inverse exact de la garantie cherchée.
 *  - JAMAIS `{ id }` en params : `SandboxFsClient.request` pose son propre id
 *    de corrélation PUIS spread les params — un `id` ici l'écraserait, le
 *    pending ne serait jamais résolu et la queue FIFO mourrait (piège vérifié
 *    en lecture de sandboxWs.ts:103-115, asserti en sandboxWs.spec). D'où le
 *    champ dédié `ackFor`.
 *
 *  Fire-and-forget : un ws fermé en vol fait rejeter le request — écho de
 *  toute façon inutile (le serveur retombe sur son timeout court).
 *
 *  Event sans id numérique → ignoré : sans `ackFor` renvoyable, le serveur
 *  n'aurait rien à résoudre (frame corrompue ; pas de flush non plus, aucune
 *  requête légitime derrière). */
export function makeFlushRequestHandler(
  getClient: () => AutosaveFsClient | null,
): (event: { id?: unknown; path?: unknown }) => void {
  return (event: { id?: unknown; path?: unknown }) => {
    if (typeof event.id !== 'number' || !Number.isInteger(event.id)) return;
    const id = event.id;
    const path = typeof event.path === 'string' ? event.path : undefined;
    // Best effort : false (aucun onglet, rien d'en attente) n'empêche pas
    // l'acquit — cf. « rien à flush est un succès » ci-dessus.
    if (path !== undefined) flushEditor(path);
    const client = getClient();
    // WS pas connecté : rien à acquitter, le serveur retombera sur timeout.
    if (!client) return;
    client.request('flush-ack', { ackFor: id }).catch(() => {});
  };
}

export function autosaveExtension(opts: {
  path: string;
  getClient: () => AutosaveFsClient | null;
  onWriteSuccess: () => void;
  onWriteError: (message: string) => void;
}): EditorAutosave {
  // Debounce « schedule-once » (patron useSandboxState) et NON le debounce à
  // timer repoussé de ideLayoutPersistence : une frappe continue repousserait
  // le timer indéfiniment et ne serait jamais écrite. La rafale déclenche un
  // timer unique ; le contenu lu à l'expiration est celui du document à ce
  // moment-là (dernier état de la rafale).
  let pending = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Vue du dernier update : `pending` ne peut devenir vrai que via un update,
  // donc elle est toujours définie quand flush() a quelque chose à vider.
  let lastView: EditorView | undefined;

  const writeNow = (view: EditorView): void => {
    pending = false;
    timer = undefined;
    const client = opts.getClient();
    // Client pas prêt (WS en cours de connexion) : écriture abandonnée
    // silencieusement — le fichier est dans le buffer, la prochaine frappe
    // réarmera le debounce (loadFile attend la même chose).
    if (!client) return;
    client
      .request<{ ok: boolean }>('write', {
        path: opts.path,
        content: view.state.doc.toString(),
      })
      .then(
        () => opts.onWriteSuccess(),
        // Échec = status (via onWriteError), PAS de retry automatique : la
        // prochaine frappe re-déclenchera. Rejet attrapé ici, jamais propagé.
        (e: unknown) => opts.onWriteError(e instanceof Error ? e.message : String(e)),
      );
  };

  const flush = (): boolean => {
    if (!pending) return false;
    clearTimeout(timer);
    const view = lastView;
    // Gardes théoriques : pending sans update n'arrive pas par construction.
    if (!view) {
      pending = false;
      timer = undefined;
      return false;
    }
    writeNow(view);
    return true;
  };

  const extension = EditorView.updateListener.of((update) => {
    lastView = update.view;
    if (!update.docChanged) return;
    // Condamné si UNE transaction de l'update porte le marqueur de reload
    // (update mixte reload+frappe : conservateur, rien n'est écrit — le
    // reload vient du disque, et une rafale mixte est le cas dégénéré).
    // `userEvent` est une annotation CM6, pas une propriété du Transaction.
    if (
      !update.transactions.every(
        (tr) => tr.annotation(Transaction.userEvent) !== DISK_RELOAD_EVENT,
      )
    ) {
      return;
    }
    if (pending) return;
    pending = true;
    timer = setTimeout(() => writeNow(update.view), AUTOSAVE_DEBOUNCE_MS);
  });

  // `hasPending` (08b) : exposition du drapeau interne `pending` — lecture
  // seule, la machine à écrire (timer/flush) reste seule maître.
  return { extension, flush, hasPending: () => pending };
}
