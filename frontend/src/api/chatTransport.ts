import type { ChatTransport, UIMessage, UIMessageChunk } from 'ai';
import type { ChatEvent } from '@vanyline/protocol/generated/chat-event';
import { chatEventsToUIStream } from '@vanyline/ui';
import { openChatWs } from './chatWs';

function textOf(message: UIMessage): string {
  return message.parts
    .filter((p): p is { type: 'text'; text: string; state?: 'streaming' | 'done' } => p.type === 'text')
    .map((p) => p.text)
    .join('');
}

/** Pont `ChatTransport` (AI SDK) — ouverture WS + délégation du mapping à
 *  `chatEventsToUIStream` (défini dans `@vanyline/ui`). */
export class VanylineChatTransport implements ChatTransport<UIMessage> {
  async sendMessages(options: {
    chatId: string;
    messages: UIMessage[];
    abortSignal: AbortSignal | undefined;
  }): Promise<ReadableStream<UIMessageChunk>> {
    const lastUser = [...options.messages].reverse().find((m) => m.role === 'user');
    const content = lastUser ? textOf(lastUser) : '';
    const ws = await openChatWs(options.chatId);

    const events = new ReadableStream<ChatEvent>({
      start(controller) {
        ws.addEventListener('message', (ev: MessageEvent) => {
          let event: ChatEvent;
          try {
            event = JSON.parse(ev.data as string) as ChatEvent;
          } catch {
            return;
          }
          // Fin du tour : le WS se ferme sur done/error (comportement actuel).
          if (event.type === 'done' || event.type === 'error') ws.close();
          controller.enqueue(event);
        });
        ws.addEventListener('error', () => controller.error(new Error('chat WebSocket error')));
        ws.addEventListener('close', () => controller.close());
        ws.send(JSON.stringify({ type: 'message', content }));
      },
      cancel() {
        ws.close();
      },
    });

    return chatEventsToUIStream(events, { abortSignal: options.abortSignal });
  }

  async reconnectToStream(_options: {
    chatId: string;
    abortSignal?: AbortSignal;
  }): Promise<ReadableStream<UIMessageChunk> | null> {
    return null;
  }
}