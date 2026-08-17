import type { ChatTransport, UIMessage, UIMessageChunk } from 'ai';
import { openChatWs } from './chatWs';

/** Miroir de `vanyline_lib::event::ChatEvent` (tag `type`, snake_case) — le
 *  protocole réel qui circule sur le WS `app` (`/api/ws/chat/{id}`). */
type ChatEvent =
  | { type: 'token'; content: string }
  | { type: 'tool_call'; id: string; name: string; args: unknown }
  | { type: 'tool_result'; id: string; name: string; result: string; is_error: boolean }
  | { type: 'skill_loaded'; name: string }
  | { type: 'subagent_start'; id: string; agent: string; task: string }
  | { type: 'subagent_event'; id: string; event: ChatEvent }
  | { type: 'subagent_end'; id: string; result: string }
  | { type: 'usage'; input_tokens: number; output_tokens: number }
  | { type: 'done' }
  | { type: 'error'; code: string; message: string }
  | { type: 'tool_unavailable'; server: string; reason: string };

function textOf(message: UIMessage): string {
  return message.parts
    .filter((p): p is { type: 'text'; text: string; state?: 'streaming' | 'done' } => p.type === 'text')
    .map((p) => p.text)
    .join('');
}

/** Pont `ChatTransport` (AI SDK) -> WS `app` (`ChatEvent`). Une connexion WS
 *  par tour (`sendMessages` == un tour) : le backend (`run_socket`, boucle
 *  sur les messages entrants) supporte aussi bien une connexion longue
 *  partagée entre tours qu'une par tour — pas de gain de latence mesurable
 *  à partager ici, et ça évite de gérer l'état d'une connexion à cheval sur
 *  plusieurs appels `sendMessages`.
 *
 *  `skill_loaded`/`subagent_*`/`usage` ne produisent aucun chunk : pas
 *  d'équivalent dans `UIMessage` (mono-agent) — risque ouvert documenté
 *  dans docs/features/chat-app-fonctionnel.md, à trancher dans une
 *  itération suivante si le multi-agent devient visible dans l'UI.
 */
export class VanylineChatTransport implements ChatTransport<UIMessage> {
  async sendMessages(options: {
    chatId: string;
    messages: UIMessage[];
    abortSignal: AbortSignal | undefined;
  }): Promise<ReadableStream<UIMessageChunk>> {
    const lastUser = [...options.messages].reverse().find((m) => m.role === 'user');
    const content = lastUser ? textOf(lastUser) : '';
    const ws = await openChatWs(options.chatId);

    let closed = false;
    const closeWs = () => {
      if (closed) return;
      closed = true;
      ws.close();
    };

    return new ReadableStream<UIMessageChunk>({
      start(controller) {
        let textId: string | null = null;
        let controllerClosed = false;
        const closeController = () => {
          if (controllerClosed) return;
          controllerClosed = true;
          try {
            controller.close();
          } catch {
            // déjà fermé côté stream (cas normal : 'close' WS après un finish/error déjà émis)
          }
        };

        options.abortSignal?.addEventListener('abort', () => {
          controller.enqueue({ type: 'abort' });
          closeController();
          closeWs();
        });

        ws.addEventListener('message', (ev: MessageEvent) => {
          let event: ChatEvent;
          try {
            event = JSON.parse(ev.data as string) as ChatEvent;
          } catch {
            return;
          }
          switch (event.type) {
            case 'token':
              if (!textId) {
                textId = crypto.randomUUID();
                controller.enqueue({ type: 'text-start', id: textId });
              }
              controller.enqueue({ type: 'text-delta', id: textId, delta: event.content });
              break;
            case 'tool_call':
              // `dynamic: true` : les noms de tools viennent du MCP de la sandbox,
              // pas d'un jeu de tools déclaré statiquement côté frontend — le
              // reducer AI SDK doit produire un `dynamic-tool` (un seul type de
              // part à gérer au rendu), pas `tool-${toolName}` par nom.
              controller.enqueue({
                type: 'tool-input-available',
                toolCallId: event.id,
                toolName: event.name,
                input: event.args,
                dynamic: true,
              });
              break;
            case 'tool_result':
              controller.enqueue({
                type: 'tool-output-available',
                toolCallId: event.id,
                output: event.result,
              });
              break;
            case 'tool_unavailable':
              controller.enqueue({
                type: 'data-tool_unavailable',
                id: crypto.randomUUID(),
                data: { server: event.server, reason: event.reason },
              });
              break;
            case 'error':
              if (textId) {
                controller.enqueue({ type: 'text-end', id: textId });
                textId = null;
              }
              controller.enqueue({ type: 'error', errorText: event.message });
              closeController();
              closeWs();
              break;
            case 'done':
              if (textId) {
                controller.enqueue({ type: 'text-end', id: textId });
                textId = null;
              }
              controller.enqueue({ type: 'finish' });
              closeController();
              closeWs();
              break;
            default:
              break;
          }
        });

        ws.addEventListener('error', () => {
          controller.enqueue({ type: 'error', errorText: 'chat WebSocket error' });
          closeController();
          closeWs();
        });

        ws.addEventListener('close', () => {
          closeController();
        });

        controller.enqueue({ type: 'start' });
        ws.send(JSON.stringify({ type: 'message', content }));
      },
      cancel() {
        closeWs();
      },
    });
  }

  async reconnectToStream(_options: {
    chatId: string;
    abortSignal?: AbortSignal;
  }): Promise<ReadableStream<UIMessageChunk> | null> {
    return null;
  }
}
