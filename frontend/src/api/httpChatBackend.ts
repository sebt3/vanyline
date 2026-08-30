import { ApiError, createApiClient } from './client';
import type { ChatBackend, ChatConversation, ChatMessageRecord } from '@vanyline/ui';
import type { PagedResult } from '../composables/useCrudResource';
import { useIdeSession } from '../composables/useIdeSession';

interface AgentOut { name: string }
interface ConversationOut { id: number }
interface ConversationRow { id: number; title: string | null; created_at: string }
interface MessageRow { id: number; role: string; payload: { content?: string }; created_at: string }

export function httpChatBackend(sandboxName: string): ChatBackend {
  const client = createApiClient();
  const { startingSession, sessionError } = useIdeSession();
  return {
    async listConversations(): Promise<ChatConversation[]> {
      const query = sandboxName ? `?sandbox_name=${encodeURIComponent(sandboxName)}` : '';
      const rows = await client.get<ConversationRow[]>(`/api/conversations${query}`);
      return rows.map((c) => ({ id: String(c.id), title: c.title, createdAt: c.created_at }));
    },
    async loadMessages(conversationId: string): Promise<ChatMessageRecord[]> {
      const rows = await client.get<MessageRow[]>(`/api/conversations/${conversationId}/messages`);
      return rows.map((m) => ({
        id: String(m.id),
        role: m.role === 'user' ? 'user' : 'assistant',
        content: m.payload.content ?? '',
      }));
    },
    async createConversation(): Promise<string> {
      sessionError.value = null;
      startingSession.value = true;
      try {
        const agentsPage = await client.get<PagedResult<AgentOut>>('/api/v1/agents');
        const agents = agentsPage.items;
        if (agents.length === 0) {
          sessionError.value = 'Aucun agent configuré — configure un agent dans Paramètres.';
          throw new Error('Aucun agent configuré — configure un agent dans Paramètres.');
        }
        const conv = await client.post<ConversationOut>('/api/conversations', {
          agent_name: agents[0].name,
          context: { kind: 'sandbox', data: { sandbox_name: sandboxName } },
        });
        return String(conv.id);
      } catch (e) {
        sessionError.value = e instanceof ApiError ? e.message : String(e);
        throw e;
      } finally {
        startingSession.value = false;
      }
    },
  };
}