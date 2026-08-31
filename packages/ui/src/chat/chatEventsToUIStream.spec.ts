import type { UIMessageChunk } from 'ai';
import type { ChatEvent } from '@vanyline/protocol/generated/chat-event';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { chatEventsToUIStream } from './chatEventsToUIStream';

describe('chatEventsToUIStream', () => {
  let c!: ReadableStreamDefaultController<ChatEvent>;
  let events: ReadableStream<ChatEvent>;
  let reader: ReadableStreamDefaultReader<UIMessageChunk>;
  const chunks: unknown[] = [];
  let stream: ReturnType<typeof chatEventsToUIStream>;

  beforeEach(async () => {
    chunks.length = 0;
    events = new ReadableStream<ChatEvent>({ start(controller) { c = controller; } });
    stream = chatEventsToUIStream(events);
    reader = stream.getReader();
    // initial read captures c and consumes { type: 'start' }
    const r = await reader.read();
    chunks.push(r.value);
  });

  afterEach(() => {
    try {
      reader.cancel();
    } catch {
      // stream may already be closed
    }
  });

  const enqueueToken = (content: string) => c.enqueue({ type: 'token', content });
  const enqueueReasoningDelta = (content: string) => c.enqueue({ type: 'reasoning_delta', content });
  const enqueueToolCall = (id: string, name: string, args: unknown) =>
    c.enqueue({ type: 'tool_call', id, name, args });
  const enqueueToolResult = (id: string, name: string, result: string, isError: boolean) =>
    c.enqueue({ type: 'tool_result', id, name, result, is_error: isError });
  const enqueueToolUnavailable = (server: string, reason: string) =>
    c.enqueue({ type: 'tool_unavailable', server, reason });
  const enqueueError = (code: string, message: string) =>
    c.enqueue({ type: 'error', code, message });
  const enqueueDone = () => c.enqueue({ type: 'done' });

  const readOne = async () => chunks.push((await reader.read()).value);

  it('token(s) puis done -> start, text-start, text-delta*, text-end, finish, stream fermé', async () => {
    enqueueToken('Bon');
    await readOne(); // text-start
    await readOne(); // text-delta "Bon"
    enqueueToken('jour');
    await readOne(); // text-delta "jour"
    enqueueDone();
    await readOne(); // text-end
    await readOne(); // finish
    const { done } = await reader.read();
    expect(done).toBe(true);

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({ type: 'text-start' });
    const textId = (chunks[1] as { id: string }).id;
    expect(chunks[2]).toEqual({ type: 'text-delta', id: textId, delta: 'Bon' });
    expect(chunks[3]).toEqual({ type: 'text-delta', id: textId, delta: 'jour' });
    expect(chunks[4]).toEqual({ type: 'text-end', id: textId });
    expect(chunks[5]).toEqual({ type: 'finish' });
  });

  it('reasoning_delta(s) puis token -> reasoning-start/delta*, reasoning-end, text-start, text-delta', async () => {
    enqueueReasoningDelta('je ré');
    await readOne(); // reasoning-start
    await readOne(); // reasoning-delta "je ré"
    enqueueReasoningDelta('fléchis');
    await readOne(); // reasoning-delta "fléchis"
    enqueueToken('Voici');
    await readOne(); // reasoning-end
    await readOne(); // text-start
    await readOne(); // text-delta "Voici"

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({ type: 'reasoning-start' });
    const reasoningId = (chunks[1] as { id: string }).id;
    expect(chunks[2]).toEqual({ type: 'reasoning-delta', id: reasoningId, delta: 'je ré' });
    expect(chunks[3]).toEqual({ type: 'reasoning-delta', id: reasoningId, delta: 'fléchis' });
    expect(chunks[4]).toEqual({ type: 'reasoning-end', id: reasoningId });
    expect(chunks[5]).toMatchObject({ type: 'text-start' });
  });

  it('deux segments de texte séparés par un tool_call -> deux text-start distincts, dans l\'ordre chronologique', async () => {
    enqueueToken('je cherche...');
    await readOne(); // text-start A
    await readOne(); // text-delta A
    enqueueToolCall('t1', 'find_file', {});
    await readOne(); // text-end A
    await readOne(); // tool-input-available
    enqueueToolResult('t1', 'find_file', 'README.md', false);
    await readOne(); // tool-output-available
    enqueueToken('voici le résumé');
    await readOne(); // text-start B
    await readOne(); // text-delta B
    enqueueDone();
    await readOne(); // text-end B
    await readOne(); // finish

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({ type: 'text-start' });
    const idA = (chunks[1] as { id: string }).id;
    expect(chunks[2]).toEqual({ type: 'text-delta', id: idA, delta: 'je cherche...' });
    expect(chunks[3]).toEqual({ type: 'text-end', id: idA });
    expect(chunks[4]).toMatchObject({ type: 'tool-input-available', toolCallId: 't1' });
    expect(chunks[5]).toMatchObject({ type: 'tool-output-available', toolCallId: 't1' });
    expect(chunks[6]).toMatchObject({ type: 'text-start' });
    const idB = (chunks[6] as { id: string }).id;
    expect(idB).not.toBe(idA);
    expect(chunks[7]).toEqual({ type: 'text-delta', id: idB, delta: 'voici le résumé' });
    expect(chunks[8]).toEqual({ type: 'text-end', id: idB });
    expect(chunks[9]).toEqual({ type: 'finish' });
  });

  it('reasoning_delta seul (pas de texte) -> reasoning-end puis finish', async () => {
    enqueueReasoningDelta('hmm');
    await readOne(); // reasoning-start
    await readOne(); // reasoning-delta
    enqueueDone();
    await readOne(); // reasoning-end
    await readOne(); // finish

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({ type: 'reasoning-start' });
    expect(chunks[2]).toMatchObject({ type: 'reasoning-delta' });
    expect(chunks[3]).toMatchObject({ type: 'reasoning-end' });
    expect(chunks[4]).toEqual({ type: 'finish' });
  });

  it('tool_call -> tool-input-available (dynamic:true), tool_result -> tool-output-available', async () => {
    enqueueToolCall('t1', 'read_file', { path: 'a' });
    await readOne(); // tool-input-available
    enqueueToolResult('t1', 'read_file', 'contenu', false);
    await readOne(); // tool-output-available

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toEqual({
      type: 'tool-input-available',
      toolCallId: 't1',
      toolName: 'read_file',
      input: { path: 'a' },
      dynamic: true,
    });
    expect(chunks[2]).toEqual({ type: 'tool-output-available', toolCallId: 't1', output: 'contenu' });
  });

  it('tool_unavailable -> data-tool_unavailable, tour non interrompu', async () => {
    enqueueToolUnavailable('sandbox', 'sandbox introuvable');
    await readOne(); // data-tool_unavailable

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({
      type: 'data-tool_unavailable',
      data: { server: 'sandbox', reason: 'sandbox introuvable' },
    });
    // Le stream est toujours ouvert — on peut continuer
    enqueueToken('suite');
    await readOne(); // text-start
    await readOne(); // text-delta
  });

  it('error -> chunk error, stream fermé (pas de finish)', async () => {
    enqueueError('VNL-LLM-001', 'boom');
    await readOne(); // error
    const { done } = await reader.read();
    expect(done).toBe(true);

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toEqual({ type: 'error', errorText: 'boom' });
  });

  it('abortSignal -> chunk abort, stream fermé', async () => {
    // Abort test needs an independent events stream (module-level is locked/used).
    // We never enqueue events to it; the abort fires during startController
    // before the stream tries to read any events.
    const abortEvents = new ReadableStream<ChatEvent>({ start: () => {} });
    const controller = new AbortController();
    const stream2 = chatEventsToUIStream(abortEvents, { abortSignal: controller.signal });
    const reader2 = stream2.getReader();
    const chunks2: unknown[] = [];
    const readOne2 = async () => chunks2.push((await reader2.read()).value);

    await readOne2(); // start
    controller.abort();
    await readOne2(); // abort
    const { done } = await reader2.read();
    expect(done).toBe(true);

    expect(chunks2[0]).toEqual({ type: 'start' });
    expect(chunks2[1]).toEqual({ type: 'abort' });
  });

  it('close() de la source sans done/error -> stream fermé', async () => {
    c.close();
    const { done } = await reader.read();
    expect(done).toBe(true);

    expect(chunks[0]).toEqual({ type: 'start' });
  });
});