import type { ChatBackend } from '@vanyline/ui';
import type { BridgeClient } from './bridge';

/** Types wire locaux — les payloads conversations ne sont pas générés ts-rs
 *  (surface RPC camelCase du CLI, cf. cli/src/rpc/handlers.rs). */
interface ConversationSummaryWire {
  id: string;
  agent?: string;
  title?: string;
  messageCount: number;
}

interface ConversationWire {
  id: string;
  agent?: string;
  title?: string;
  messages: Array<{ role: string; content: string }>;
}

/** ChatBackend relayé en `conversations/list|get|create` via le pont postMessage. */
export function createBridgeBackend(bridge: BridgeClient): ChatBackend {
  return {
    async listConversations() {
      // Écart design assumé — tranchage développeur du 2026-09-03 : la surface RPC
      // F2 (store CLI) ne porte AUCUN timestamp, alors que ChatConversation du port
      // exige `createdAt` et que ChatWindow retombe sur `Session du <date>` sans
      // titre. Garantie côté webview : title = title ?? 'Session <id8>' (même
      // convention que le QuickPick du host, tâche 04a) et createdAt = '' — le
      // repli date n'est jamais atteint. Backlog (hors F3) : horodatage des
      // conversations dans le store CLI.
      const summaries = await bridge.request<ConversationSummaryWire[]>(
        'conversations/list',
        {},
      );
      return summaries.map((s) => ({
        id: s.id,
        title: s.title ?? 'Session ' + s.id.slice(0, 8),
        createdAt: '',
      }));
    },

    async loadMessages(conversationId) {
      const conv = await bridge.request<ConversationWire>('conversations/get', {
        id: conversationId,
      });
      // Seuls user/assistant sont affichables ; id = index dans la liste filtrée.
      return conv.messages
        .filter(
          (m): m is { role: 'user' | 'assistant'; content: string } =>
            m.role === 'user' || m.role === 'assistant',
        )
        .map((m, index) => ({ id: String(index), role: m.role, content: m.content }));
    },

    async createConversation() {
      const result = await bridge.request<{ id: string }>('conversations/create', {});
      return result.id;
    },
  };
}
