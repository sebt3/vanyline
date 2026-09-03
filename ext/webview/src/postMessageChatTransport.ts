import type { ChatTransport, UIMessage, UIMessageChunk } from 'ai';
import type { ChatEvent } from '@vanyline/protocol/generated/chat-event';
import { chatEventsToUIStream } from '@vanyline/ui';
import type { BridgeClient } from './bridge';

/** Texte d'un message UI : concaténation des parts `text` (pattern de
 *  frontend/src/api/chatTransport.ts). */
function textOf(message: UIMessage): string {
  return message.parts
    .filter(
      (p): p is { type: 'text'; text: string; state?: 'streaming' | 'done' } =>
        p.type === 'text',
    )
    .map((p) => p.text)
    .join('');
}

/** ChatTransport sur le pont postMessage — même forme que le transport frontend
 *  (frontend/src/api/chatTransport.ts), mapping délégué à chatEventsToUIStream. */
export class PostMessageChatTransport implements ChatTransport<UIMessage> {
  private readonly bridge: BridgeClient;
  private readonly getAgent: () => string | undefined;

  constructor(bridge: BridgeClient, getAgent: () => string | undefined) {
    this.bridge = bridge;
    this.getAgent = getAgent;
  }

  async sendMessages(options: {
    chatId: string;
    messages: UIMessage[];
    abortSignal: AbortSignal | undefined;
  }): Promise<ReadableStream<UIMessageChunk>> {
    const bridge = this.bridge;
    const getAgent = this.getAgent;
    const chatId = options.chatId;

    // Le texte envoyé = les parts text du DERNIER message user.
    const lastUser = [...options.messages].reverse().find((m) => m.role === 'user');
    const message = lastUser ? textOf(lastUser) : '';

    let unsubscribe: (() => void) | undefined;

    const events = new ReadableStream<ChatEvent>({
      start(controller) {
        unsubscribe = bridge.onChatEvent((p) => {
          // Notification globale : filtrer les autres conversations chez nous.
          if (p.conversationId !== chatId) return;
          controller.enqueue(p.event);
          if (p.event.type === 'done' || p.event.type === 'error') {
            unsubscribe?.();
            controller.close();
          }
        });

        // Échec de validation du tour (ok:false) → stream en erreur. La résolution
        // = fin de tour : le stream est déjà fermé par l'événement `done` → ignorer.
        bridge
          .chatSend({ conversationId: chatId, message, agent: getAgent() })
          .catch((err: unknown) => {
            controller.error(err);
          });

        if (options.abortSignal) {
          options.abortSignal.addEventListener('abort', () => {
            void bridge.chatCancel(chatId).catch(() => {});
          });
        }
      },
      cancel() {
        // Flux consommé annulé : on se désabonne et on demande l'annulation du tour.
        unsubscribe?.();
        void bridge.chatCancel(chatId).catch(() => {});
      },
    });

    return chatEventsToUIStream(events, { abortSignal: options.abortSignal });
  }

  async reconnectToStream(): Promise<null> {
    // Pas de reprise de stream (le tour vit dans le process CLI, pas resumed ici).
    return null;
  }
}
