import * as vscode from 'vscode';
import { spawn as cpSpawn } from 'node:child_process';
import { registerChatView } from './panels/chat';
import { startServer, type ServerHandle } from './rpc';

let handle: ServerHandle | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  registerChatView(context);

  const channel = vscode.window.createOutputChannel('vanyline');
  context.subscriptions.push(channel);

  const cfg = vscode.workspace.getConfiguration('vanyline');
  const bin = cfg.get<string>('serverPath', '').trim() || 'vanyline';
  channel.appendLine(`binaire vanyline : ${bin}`); // « log clair du binaire utilisé » (design)

  try {
    handle = await startServer({
      spawn: cpSpawn,
      channel,
      bin,
      workspace: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
      logLevel: cfg.get<string>('defaultLogLevel', 'info'),
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    channel.appendLine(msg);
    void vscode.window.showErrorMessage(msg);
    // activation dégradée : la vue reste enregistrée, pas de throw (design « offline »)
  }
}

export async function deactivate(): Promise<void> {
  if (!handle) return;
  try {
    await handle.dispose();
  } catch {
    /* mort déjà : rien à faire */
  }
  handle = undefined;
}
