import * as vscode from 'vscode';
import { buildHtml, generateNonce } from './html';
import { handleBridgeRequest, type BridgeApi, type ChatEventMessage } from './bridge';
import type { LogChannel, ServerHandle } from '../rpc';

export class ChatViewProvider implements vscode.WebviewViewProvider {
  private view?: vscode.WebviewView;
  private handle?: ServerHandle;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly channel: LogChannel,
  ) {}

  /**
   * Branche (ou re-branche) le provider sur un handle vivant : l'abonnement
   * `chat/event` est unique par handle — ré-appeler avec le même handle est un no-op ;
   * un handle différent (restart) = ré-abonnement sur la nouvelle connexion, l'ancien
   * handle étant de toute façon disposed.
   */
  attachServer(handle: ServerHandle): void {
    if (this.handle === handle) {
      return;
    }
    this.handle = handle;
    handle.conn.onNotification('chat/event', (params) => {
      // params = {conversationId, seq, event} — notification de la CLI, relayée telle quelle.
      this.post({ type: 'chat/event', ...(params as Omit<ChatEventMessage, 'type'>) });
    });
  }

  /** Handle parti (stop / fenêtre de restart) : les requêtes webview répondront -021. */
  detachServer(): void {
    this.handle = undefined;
  }

  /** Message hôte → webview (session/new, session/pick, chat/event). */
  post(msg: Record<string, unknown>): void {
    void this.view?.webview.postMessage(msg);
  }

  resolveWebviewView(webviewView: vscode.WebviewView): void {
    this.view = webviewView;

    const distWebview = vscode.Uri.joinPath(
      this.context.extensionUri,
      'dist',
      'webview',
    );

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [distWebview],
    };

    webviewView.webview.html = buildHtml(
      webviewView.webview.asWebviewUri(distWebview).toString(),
      webviewView.webview.cspSource,
      generateNonce(),
    );

    webviewView.webview.onDidReceiveMessage((raw: unknown) => {
      void handleBridgeRequest(raw, this.bridgeApi(), this.handle !== undefined);
    });

    if (this.handle) {
      // Vue recréée alors qu'un handle était déjà attaché : le handler de notification
      // passe par `this.view` (dynamique), et ce ré-appel est un no-op pour un handle
      // identique — l'abonnement est vivant et cible la nouvelle vue, sans doublon.
      this.attachServer(this.handle);
    }
  }

  /** BridgeApi du module pur (bridge.ts) : relais vers le handle courant, réponses
   *  vers la webview, journal vers l'OutputChannel. */
  private bridgeApi(): BridgeApi {
    return {
      request: <T>(method: string, params?: unknown): Promise<T> =>
        this.handle!.conn.request<T>(method, params),
      respond: (resp) => this.post(resp),
      log: (line) => this.channel.appendLine(line),
    };
  }

  show(): void {
    if (this.view) {
      this.view.show(true);
    } else {
      void vscode.commands.executeCommand('vanyline.chatView.focus');
    }
  }
}

/** Enregistre le provider (retainContextWhenHidden: true) + la commande vanyline.openPanel. */
export function registerChatView(
  context: vscode.ExtensionContext,
  channel: LogChannel,
): ChatViewProvider {
  const provider = new ChatViewProvider(context, channel);

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider('vanyline.chatView', provider, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.openPanel', () => provider.show()),
  );

  return provider;
}
