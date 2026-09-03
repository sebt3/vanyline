import * as vscode from 'vscode';
import { spawn as cpSpawn } from 'node:child_process';
import { registerChatView } from './panels/chat';
import { startServer } from './rpc';
import { createSupervisor, type Supervisor } from './supervisor';

let supervisor: Supervisor | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  registerChatView(context);

  const channel = vscode.window.createOutputChannel('vanyline');
  const statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBar.command = 'vanyline.restartServer';
  statusBar.show();
  context.subscriptions.push(channel, statusBar);

  supervisor = createSupervisor({
    channel,
    statusBar: {
      setText: (t) => {
        statusBar.text = t;
      },
    },
    start: async () => {
      // relecture de la config À CHAQUE tentative (serverPath changeable sans reload)
      const cfg = vscode.workspace.getConfiguration('vanyline');
      const bin = cfg.get<string>('serverPath', '').trim() || 'vanyline';
      channel.appendLine(`binaire vanyline : ${bin}`); // « log clair du binaire utilisé » (design)
      return startServer({
        spawn: cpSpawn,
        channel,
        bin,
        workspace: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
        logLevel: cfg.get<string>('defaultLogLevel', 'info'),
      });
    },
    notifyError: (msg) => {
      channel.appendLine(msg);
      void vscode.window.showErrorMessage(msg);
    },
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.restartServer', () => supervisor?.restart()),
  );

  await supervisor.start(); // ne rejette jamais (activation dégradée, design « offline »)
}

export async function deactivate(): Promise<void> {
  await supervisor?.stop();
}
