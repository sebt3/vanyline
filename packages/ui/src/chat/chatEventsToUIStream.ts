import type { UIMessageChunk } from 'ai';
import type { ChatEvent } from '@vanyline/protocol/generated/chat-event';

export function chatEventsToUIStream(
  events: ReadableStream<ChatEvent>,
  options?: { abortSignal?: AbortSignal },
): ReadableStream<UIMessageChunk> {
  let c: ReadableStreamDefaultController<UIMessageChunk>;
  let controllerClosed = false;

  return new ReadableStream<UIMessageChunk>({
    start(ctrl) {
      c = ctrl;
      ctrl.enqueue({ type: 'start' });

      const reader = events.getReader();
      let textId: string | null = null;
      let reasoningId: string | null = null;
      let doneEmitted = false;

      const closeText = () => {
        if (textId) {
          ctrl.enqueue({ type: 'text-end', id: textId });
          textId = null;
        }
      };
      const closeReasoning = () => {
        if (reasoningId) {
          ctrl.enqueue({ type: 'reasoning-end', id: reasoningId });
          reasoningId = null;
        }
      };
      const closeOpenBlocks = () => {
        closeReasoning();
        closeText();
      };

      const enqueue = (chunk: UIMessageChunk) => ctrl.enqueue(chunk);

      const handleEvent = (event: ChatEvent) => {
        switch (event.type) {
          case 'reasoning_delta':
            closeText();
            if (!reasoningId) {
              reasoningId = crypto.randomUUID();
              enqueue({ type: 'reasoning-start', id: reasoningId });
            }
            enqueue({ type: 'reasoning-delta', id: reasoningId, delta: event.content });
            break;
          case 'token':
            closeReasoning();
            if (!textId) {
              textId = crypto.randomUUID();
              enqueue({ type: 'text-start', id: textId });
            }
            enqueue({ type: 'text-delta', id: textId, delta: event.content });
            break;
          case 'tool_call':
            closeOpenBlocks();
            enqueue({
              type: 'tool-input-available',
              toolCallId: event.id,
              toolName: event.name,
              input: event.args,
              dynamic: true,
            });
            break;
          case 'tool_result':
            closeOpenBlocks();
            enqueue({
              type: 'tool-output-available',
              toolCallId: event.id,
              output: event.result,
            });
            break;
          case 'tool_unavailable':
            closeOpenBlocks();
            enqueue({
              type: 'data-tool_unavailable',
              id: crypto.randomUUID(),
              data: { server: event.server, reason: event.reason },
            });
            break;
          case 'error':
            closeOpenBlocks();
            enqueue({ type: 'error', errorText: event.message });
            closeController();
            doneEmitted = true;
            break;
          case 'done':
            closeOpenBlocks();
            enqueue({ type: 'finish' });
            closeController();
            doneEmitted = true;
            break;
        }
      };

      const controller = () => {
        if (controllerClosed) return;
        controllerClosed = true;
        try {
          ctrl.close();
        } catch {
          // already closed
        }
      };
      const closeController = controller;

      const abortSignal = options?.abortSignal;
      if (abortSignal) {
        if (abortSignal.aborted) {
          enqueue({ type: 'abort' });
          closeController();
          try {
            reader.cancel();
          } catch {
            // stream may already be closed
          }
          return;
        }
        abortSignal.addEventListener('abort', () => {
          enqueue({ type: 'abort' });
          closeController();
          try {
            reader.cancel();
          } catch {
            // stream may already be closed
          }
        });
      }

      void reader.read().then(function handleRead(
        result: ReadableStreamReadResult<ChatEvent>,
      ): Promise<void> | void {
        if (result.done) {
          if (!doneEmitted) {
            closeController();
          }
          return;
        }
        handleEvent(result.value);
        return reader.read().then(handleRead);
      }).catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        ctrl.enqueue({ type: 'error', errorText: msg });
        closeController();
      });
    },
    cancel() {
      try {
        c.close();
      } catch {
        // readable stream may already be closed
      }
    },
  });
}