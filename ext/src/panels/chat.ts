import * as vscode from 'vscode';
import { buildHtml, generateNonce } from './html';

export class ChatViewProvider implements vscode.WebviewViewProvider {
  private view?: vscode.WebviewView;

  constructor(private readonly context: vscode.ExtensionContext) {}

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
export function registerChatView(context: vscode.ExtensionContext): ChatViewProvider {
  const provider = new ChatViewProvider(context);

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
