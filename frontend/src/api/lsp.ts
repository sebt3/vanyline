import type { LSPClient, Transport } from '@codemirror/lsp-client';
import { LSPClient as LSPClientClass, languageServerExtensions } from '@codemirror/lsp-client';
import { openSandboxWs } from './sandboxWs';
import { VanylineWorkspace } from './lspWorkspace';

/** Transport JSON-RPC sur le WS sandbox : un message JSON par frame texte (contrat
 *  `sandbox/src/ws/lsp.rs`), pas de framing Content-Length. `send` throw si le WS est
 *  fermé (doc `Transport` du package). `subscribe`/`unsubscribe` sur une liste de
 *  handlers, `ws.onmessage` route chaque frame texte vers tous les handlers. */
function wsTransport(ws: WebSocket): Transport {
  const handlers: Array<(data: string) => void> = [];
  const onMessage = (ev: MessageEvent) => {
    const msg = typeof ev.data === 'string' ? ev.data : JSON.stringify(ev.data);
    for (const h of handlers) h(msg);
  };
  ws.addEventListener('message', onMessage as EventListener);
  return {
    send: (data: string) => ws.send(data),
    subscribe: (handler) => handlers.push(handler),
    unsubscribe: (handler) => {
      const idx = handlers.indexOf(handler);
      if (idx !== -1) handlers.splice(idx, 1);
    },
  };
}

/** Internal caches — clés `{sandbox}/{toolchain}`. */
const sockets = new Map<string, WebSocket>();
const clients = new Map<string, LSPClient>();
const cache = new Map<string, Promise<LSPClient | null>>();

/** Ouvre la connexion `/ws/lsp/{toolchain}`, connecte un `LSPClient`
 *  ({ extensions: languageServerExtensions(), timeout: 30_000 }), attend
 *  `client.initializing` et rend le client. Rejette si ticket/WS/init échoue.
 *
 *  `rootUri` : le package ne le déduit jamais lui-même — `LSPClientConfig.rootUri`
 *  vaut `null` si on ne le fournit pas (vérifié dans la source du package). Laissé
 *  `null`, le process retombe sur son cwd de spawn (`sandbox_root`, la racine du
 *  monorepo) pour chercher un `tsserver`/`typescript` local — ça ne marche que si le
 *  projet du langage vit exactement à cette racine (le cas de rust ici, par hasard :
 *  `Cargo.toml` y est). Pour un projet dans un sous-répertoire (`frontend/` pour
 *  node/ts), le process ne le trouve jamais avec `rootUri: null` même si
 *  `npm install` a bien été fait — trouvé en usage réel. D'où le répertoire du
 *  premier fichier ouvert comme racine : les serveurs LSP TS/JS remontent
 *  l'arborescence depuis la racine fournie pour trouver le projet, un répertoire
 *  sous ce projet (pas forcément exactement dessus) suffit. */
async function openAndConnect(
  sandboxName: string,
  toolchain: string,
  rootUri: string,
  openFile?: (path: string) => void,
): Promise<LSPClient> {
  const key = `${sandboxName}/${toolchain}`;
  const ws = await openSandboxWs(sandboxName, `/ws/lsp/${toolchain}`);
  sockets.set(key, ws);

  const client = new LSPClientClass({
    extensions: languageServerExtensions(),
    timeout: 30_000,
    rootUri,
    workspace: (client) => new VanylineWorkspace(client, openFile),
  }).connect(wsTransport(ws));

  await client.initializing;
  clients.set(key, client);
  return client;
}

/** Get-or-create : une seule connexion par `{sandbox}/{toolchain}` (cache de promesses).
 *  Échec → `null` (pas de retry, le cache mémorise l'échec). `rootUri` n'a d'effet
 *  qu'à la création (premier appel pour ce `{sandbox, toolchain}`) — la session est
 *  partagée, cf. `openAndConnect`. */
export async function getLspClient(
  sandboxName: string,
  toolchain: string,
  rootUri: string,
  openFile?: (path: string) => void,
): Promise<LSPClient | null> {
  const key = `${sandboxName}/${toolchain}`;
  const existing = cache.get(key);
  if (existing) return existing;

  const promise = openAndConnect(sandboxName, toolchain, rootUri, openFile).catch(() => null);
  cache.set(key, promise);
  return promise;
}

/** Ferme les connexions LSP de `sandboxName` (disconnect + ws.close) et vide le cache
 *  correspondant — appelé au démontage de l'IDE. Les connexions en vol sont fermées
 *  aussi (leur ws est enregistré dès l'ouverture) ; si elles résolvent après le dispose,
 *  le client est connecté sur un ws fermé → les appels LSP échoueront (mode dégradé). */
export function disposeLspClients(sandboxName: string): void {
  const prefix = `${sandboxName}/`;
  for (const [key, ws] of sockets) {
    if (!key.startsWith(prefix)) continue;
    clients.get(key)?.disconnect();
    ws.close();
    sockets.delete(key);
    clients.delete(key);
    cache.delete(key);
  }
}

/** Internals exposés uniquement pour les tests, afin de vider le cache
 *  et la map de sockets entre les tests (les Maps sont privées, inaccessible
 *  depuis l'extérieur). */
export function __testReset(): void {
  cache.clear();
  sockets.clear();
  clients.clear();
}