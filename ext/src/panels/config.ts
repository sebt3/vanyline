import * as vscode from 'vscode';
import { buildHtml, generateNonce } from './html';
import { handleBridgeRequest, type BridgeApi } from './bridge';
import type { LogChannel, ServerHandle } from '../rpc';

/** Ouvre (ou révèle) le panel de configuration. Retourne l'instance pour
 *  l'extension (relais webview→CLI branché en tâche 06b ; broadcast host→webview
 *  via `post` depuis la tâche 07). */
export interface ConfigPanelHandle {
  open(): void;
  /** Branche le panel sur un handle vivant (aucun abonnement notification — le
   *  handle ne sert qu'au relais des requêtes, config/changed naît des writes
   *  vus par bridge.ts, pas d'une notification CLI) ; no-op si handle identique. */
  attachServer(handle: ServerHandle): void;
  /** Handle parti (restart) : les requêtes webview répondront VNL-EXT-021. */
  detachServer(): void;
  /** Message host → webview du panel (broadcast config/changed). No-op si
   *  aucun panel ouvert. */
  post(msg: Record<string, unknown>): void;
}

/** Enregistre la logique du panel ; `open()` crée le panel au premier appel,
 *  `reveal()` sur les appels suivants (panel unique — design « panel unique
 *  réutilisé »). `onDidDispose` remet l'état à zéro pour permettre une
 *  réouverture après fermeture. `onConfigWrite` (optionnel, tâche 07) : callback
 *  de broadcast config/changed, même contrat que registerChatView. */
export function registerConfigPanel(
  context: vscode.ExtensionContext,
  channel: LogChannel,
  onConfigWrite?: (domain: string) => void,
): ConfigPanelHandle {
  let panel: vscode.WebviewPanel | undefined;
  // Handle du superviseur : variable de CLOSURE du module registerConfigPanel, PAS du
  // panel. Un handle attaché alors qu'aucun panel n'existe est simplement retenu
  // (bridgeApi() le lit à la demande — lecture dynamique, cf. le commentaire de
  // chat.ts:66-71) ; un panel fermé puis rouvert continue d'être servi par le handle
  // déjà attaché, le handle suivant le cycle de vie du serveur et non celui du panel.
  let handle: ServerHandle | undefined;

  function attachServer(h: ServerHandle): void {
    if (handle === h) {
      return;
    }
    handle = h;
    // Aucun abonnement notification à poser ici : contrairement à chat.ts, rien
    // n'accroche le handle — bridgeApi() le relit à chaque message. config/changed
    // ne vient pas d'une notification CLI mais des writes vus par bridge.ts
    // (onWriteSucceeded), diffusés par extension.ts via post().
  }

  /** Handle parti (fenêtre de restart) : les requêtes webview répondront -021. */
  function detachServer(): void {
    handle = undefined;
  }

  /** Message hôte → webview du panel (broadcast config/changed, tâche 07).
   *  No-op si aucun panel ouvert : `panel?.webview`, pas de crash possible. */
  function post(msg: Record<string, unknown>): void {
    void panel?.webview.postMessage(msg);
  }

  /** BridgeApi du module pur (bridge.ts) — mêmes champs que chat.ts : relais vers le
   *  handle courant (lecture dynamique à l'appel, jamais capturé), réponses vers la
   *  webview du panel courant, journal vers l'OutputChannel, broadcast config/changed
   *  via onWriteSucceeded (le champ optionnel du BridgeApi accepte undefined). */
  function bridgeApi(): BridgeApi {
    return {
      request: <T>(method: string, params?: unknown): Promise<T> =>
        handle!.conn.request<T>(method, params),
      respond: (resp) => {
        void panel?.webview.postMessage(resp);
      },
      log: (line) => channel.appendLine(line),
      onWriteSucceeded: onConfigWrite,
    };
  }

  function open(): void {
    if (panel) {
      panel.reveal(vscode.ViewColumn.Active);
      return;
    }

    const distWebview = vscode.Uri.joinPath(
      context.extensionUri,
      'dist',
      'webview',
    );

    panel = vscode.window.createWebviewPanel(
      'vanyline.config',
      'vanyline — Configuration',
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        localResourceRoots: [distWebview],
        retainContextWhenHidden: true,
      },
    );

    panel.webview.html = buildHtml(
      panel.webview.asWebviewUri(distWebview).toString(),
      panel.webview.cspSource,
      generateNonce(),
      'config',
    );

    // Webview → host : parse + whitelist + relais par le module pur (bridge.ts,
    // même logique que chat.ts:62-64). `handle !== undefined` = serveur vivant ;
    // sinon bridge.ts répond -021 sans jamais toucher api.request. Le `handle` lu
    // est celui de la closure : une réouverture après fermeture retrouve le handle
    // déjà attaché (le panel ne porte aucun état de connexion).
    panel.webview.onDidReceiveMessage((raw: unknown) => {
      void handleBridgeRequest(raw, bridgeApi(), handle !== undefined);
    });

    // Fermeture → état à zéro pour permettre une réouverture propre (panel unique).
    // Seule la référence au panel est effacée : `handle` (closure) survit à la
    // fermeture, c'est le superviseur qui seul attache/détache.
    panel.onDidDispose(() => {
      channel.appendLine('panel de configuration fermé');
      panel = undefined;
    });
  }

  return { open, attachServer, detachServer, post };
}
