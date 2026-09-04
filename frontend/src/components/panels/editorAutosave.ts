import { EditorView } from '@codemirror/view';
import { Transaction } from '@codemirror/state';
import type { Extension } from '@codemirror/state';

/**
 * Autosave éditeur (feature lsp-agent-interface, tâche 07a).
 *
 * Une extension CodeMirror par onglet : document modifié → `write` debouncé
 * vers /ws/fs, exactement la payload du save manuel (`Editor.vue`), sérialisée
 * par la queue du `SandboxFsClient` (le serveur est strictement
 * requête→réponse : écrire hors de cette queue corromprait le protocole).
 *
 * Le canal « fichier changé sur disque » (frame serveur + reload câblé) est
 * reporté à la tâche 08 — aucune frame non sollicitée ne peut transiter sur
 * /ws/fs tel quel. Ce module pose néanmoins ses deux briques client :
 * `DISK_RELOAD_EVENT` (marqueur anti-boucle déjà respecté par l'autosave) et
 * `applyDiskReload`, plus le registre path→flush dont elle aura besoin pour
 * « flush avant écriture LLM ».
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
 *  double écriture de save(), cf. Editor.vue). */
export type EditorAutosave = Extension & {
  flush: () => boolean;
};

// Registre module path→flush : un seul enregistrement par path (dernier
// gagne) — un onglet = un path = une vue.
const flushRegistry = new Map<string, () => void>();

/** Enregistre le flush d'une instance d'éditeur par path ; renvoie la fonction
 *  de désenregistrement (appelée au démontage de l'instance).
 *
 *  Le désenregistrement est gardé par identité : si un nouvel enregistrement
 *  a remplacé celui-ci (même path, instance recréée avant démontage de
 *  l'ancienne), l'unregister tardif de l'ancienne ne doit pas la débrancher. */
export function registerEditorFlush(path: string, flush: () => void): () => void {
  flushRegistry.set(path, flush);
  return () => {
    if (flushRegistry.get(path) === flush) flushRegistry.delete(path);
  };
}

/** Vide le registre : notifie le flush de TOUS les éditeurs enregistrés.
 *  Utilisé par la tâche 08 (flush global avant édition LLM) ; ici testé seul.
 *  Copie itérative : un flush qui se désenregistre lui-même ne doit pas
 *  fausser la boucle. */
export function flushAllEditors(): void {
  for (const flush of [...flushRegistry.values()]) flush();
}

/** Recharge le buffer d'une vue depuis un contenu venant du disque :
 *  transaction full-doc, `userEvent: DISK_RELOAD_EVENT`. Ne fait AUCUNE
 *  écriture, AUCUNE lecture. (La tâche 08 l'appellera sur réception de la
 *  future frame serveur.) */
export function applyDiskReload(view: EditorView, content: string): void {
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: content },
    userEvent: DISK_RELOAD_EVENT,
  });
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

  return { extension, flush };
}
