import * as vscode from 'vscode';
import { registerChatView } from './panels/chat';

export function activate(context: vscode.ExtensionContext): void {
  registerChatView(context);
}

/** no-op — le spawn RPC arrive en tâche 02. */
export function deactivate(): void {}
