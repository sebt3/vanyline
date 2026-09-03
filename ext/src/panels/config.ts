import * as vscode from 'vscode';
import { buildHtml, generateNonce } from './html';
import type { LogChannel } from '../rpc';

/** Ouvre (ou révèle) le panel de configuration. Retourne l'instance pour
 *  l'extension (relais + config/changed branchés en tâche 06). */
export interface ConfigPanelHandle {
  open(): void;
}

/** Enregistre la logique du panel ; `open()` crée le panel au premier appel,
 *  `reveal()` sur les appels suivants (panel unique — design « panel unique
 *  réutilisé »). `onDidDispose` remet l'état à zéro pour permettre une
 *  réouverture après fermeture. */
export function registerConfigPanel(
  context: vscode.ExtensionContext,
  channel: LogChannel,
): ConfigPanelHandle {
  let panel: vscode.WebviewPanel | undefined;

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

    // Fermeture → état à zéro pour permettre une réouverture propre (panel unique).
    panel.onDidDispose(() => {
      channel.appendLine('panel de configuration fermé');
      panel = undefined;
    });
  }

  return { open };
}
