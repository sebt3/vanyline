import type { ChatTransport, UIMessage } from 'ai';

export interface ChatConversation {
  id: string;
  title: string | null;
  createdAt: string;
}

export interface ChatMessageRecord {
  id: string;
  role: 'user' | 'assistant';
  content: string;
}

export interface ChatBackend {
  /** Conversations du sélecteur, plus récentes d'abord. */
  listConversations(): Promise<ChatConversation[]>;
  /** Historique persisté d'une conversation, ordre chronologique. Vide pour une neuve. */
  loadMessages(conversationId: string): Promise<ChatMessageRecord[]>;
  /** Crée une conversation, renvoie son id. L'impl porte son propre contexte
   *  (web = contexte sandbox + résolution du 1ᵉʳ agent ; CLI (F4) = agent du YAML).
   *  Aucun paramètre côté port — la politique de contexte ne remonte jamais
   *  dans les composants. */
  createConversation(): Promise<string>;
}

export type { ChatTransport, UIMessage };