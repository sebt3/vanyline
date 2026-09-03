import * as vscode from 'vscode';
import { spawn as cpSpawn } from 'node:child_process';
import { ensureCli, productionDeps } from './cli-provisioning';
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
      // relecture de la config À CHAQUE tentative (serverPath/autoUpdate changeables sans reload)
      const cfg = vscode.workspace.getConfiguration('vanyline');
      const resolved = await ensureCli(
        {
          serverPath: cfg.get<string>('serverPath', ''),
          autoUpdate: cfg.get<boolean>('autoUpdateCli', true),
        },
        productionDeps((line) => channel.appendLine(line)),
      );
      channel.appendLine(`binaire vanyline : ${resolved.bin} (${resolved.source})`); // « log clair du binaire utilisé » (design)
      return startServer({
        spawn: cpSpawn,
        channel,
        bin: resolved.bin,
        workspace: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
        logLevel: cfg.get<string>('defaultLogLevel', 'info'),
      });
    },
    // PAS d'appendLine ici : le superviseur journalise déjà chaque message qu'il affiche
    // (reportError) — le coller ici écrirait chaque erreur deux fois dans le canal.
    notifyError: (msg) => void vscode.window.showErrorMessage(msg),
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.restartServer', () => supervisor?.restart()),
  );

  await supervisor.start(); // ne rejette jamais (activation dégradée, design « offline »)
}

export async function deactivate(): Promise<void> {
  await supervisor?.stop();
}
