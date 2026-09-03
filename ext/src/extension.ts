import * as vscode from 'vscode';
import { spawn as cpSpawn } from 'node:child_process';
import type { ConversationSummary } from '@vanyline/protocol';
import { ensureCli, productionDeps } from './cli-provisioning';
import { mapRpcError } from './panels/bridge';
import { registerChatView } from './panels/chat';
import { registerConfigPanel } from './panels/config';
import { startServer } from './rpc';
import { createSupervisor, type Supervisor } from './supervisor';

let supervisor: Supervisor | undefined;

/** VNL-EXT-021 : commandes exigeant le serveur alors qu'aucun handle n'est vivant. */
const SERVER_NOT_STARTED =
  'VNL-EXT-021: serveur vanyline non démarré (voir vanyline.restartServer)';

/** Message user-facing d'un relais RPC en échec : cite l'identifiant serveur si connu. */
function rpcErrorMessage(action: string, err: unknown): string {
  const mapped = mapRpcError(err);
  return `vanyline: ${action} (${mapped.code ? `${mapped.code} — ` : ''}${mapped.message})`;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const channel = vscode.window.createOutputChannel('vanyline');

  // Broadcast config/changed (tâche 07) : les deux fournisseurs sont enregistrés avant
  // d'être câblés entre eux (le panel est créé après le provider) — la closure
  // late-bound est délibérée, pas un ordre à « corriger ». Les callbacks passés aux
  // deux register* ne référencent que la variable mutable `broadcastConfigChanged`,
  // réassignée après les deux enregistrements (TS strict interdit d'utiliser une
  // const non encore définie — d'où le placeholder no-op).
  let broadcastConfigChanged: (domain: string) => void = () => {};
  const provider = registerChatView(context, channel, (d) => broadcastConfigChanged(d));
  const configPanel = registerConfigPanel(context, channel, (d) => broadcastConfigChanged(d));
  broadcastConfigChanged = (domain: string): void => {
    const msg = { type: 'config/changed', domain };
    provider.post(msg);
    configPanel.post(msg);
  };

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

  // Provider chat + panel config ↔ superviseur, AVANT start() pour ne manquer le premier
  // 'ready' : le handle change à chaque restart → re-attach à chaque 'ready', detach
  // ailleurs (les deux ponts répondent -021 — mêmes sémantiques, même fenêtre de restart).
  supervisor.onStatus((s) => {
    if (s === 'ready') {
      const h = supervisor?.current();
      if (h) {
        provider.attachServer(h);
        configPanel.attachServer(h);
      }
    } else {
      provider.detachServer();
      configPanel.detachServer();
    }
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('vanyline.restartServer', () => supervisor?.restart()),

    vscode.commands.registerCommand('vanyline.newSession', async () => {
      provider.show();
      const h = supervisor?.current();
      if (!h) {
        void vscode.window.showErrorMessage(SERVER_NOT_STARTED);
        return;
      }
      try {
        const result = await h.conn.request<{ id: string }>('conversations/create', {});
        provider.post({ type: 'session/new', conversationId: result.id });
      } catch (err) {
        void vscode.window.showErrorMessage(rpcErrorMessage('création de session impossible', err));
      }
    }),

    vscode.commands.registerCommand('vanyline.sessionPicker', async () => {
      const h = supervisor?.current();
      if (!h) {
        void vscode.window.showErrorMessage(SERVER_NOT_STARTED);
        return;
      }
      let sessions: ConversationSummary[];
      try {
        sessions = await h.conn.request<ConversationSummary[]>('conversations/list');
      } catch (err) {
        void vscode.window.showErrorMessage(rpcErrorMessage('liste des sessions impossible', err));
        return;
      }
      const picked = await vscode.window.showQuickPick(
        sessions.map((s) => ({
          label: s.title ?? `Session ${s.id.slice(0, 8)}`,
          description: `${String(s.messageCount)} message(s)`,
          id: s.id,
        })),
      );
      provider.show();
      provider.post({ type: 'session/pick', conversationId: picked?.id ?? null });
    }),

    vscode.commands.registerCommand('vanyline.openSettings', () => configPanel.open()),
  );

  await supervisor.start(); // ne rejette jamais (activation dégradée, design « offline »)
}

export async function deactivate(): Promise<void> {
  await supervisor?.stop();
}
