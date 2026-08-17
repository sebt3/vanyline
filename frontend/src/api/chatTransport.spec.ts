import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UIMessage } from 'ai';
import { VanylineChatTransport } from './chatTransport';

const { wsInstances } = vi.hoisted(() => ({
  wsInstances: [] as Array<{
    listeners: Record<string, Array<(ev: { data?: unknown }) => void>>;
    sent: string[];
    close: () => void;
    send: (data: string) => void;
    emit: (type: string, data?: unknown) => void;
  }>,
}));

vi.mock('./chatWs', () => ({
  openChatWs: vi.fn(() => {
    const listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
    const instance = {
      listeners,
      sent: [] as string[],
      close: vi.fn(),
      send: vi.fn(function (this: { sent: string[] }, data: string) {
        this.sent.push(data);
      }),
      addEventListener(type: string, cb: (ev: { data?: unknown }) => void) {
        (listeners[type] ??= []).push(cb);
      },
      emit(type: string, data?: unknown) {
        for (const cb of [...(listeners[type] ?? [])]) cb({ data });
      },
    };
    wsInstances.push(instance as unknown as (typeof wsInstances)[number]);
    return Promise.resolve(instance);
  }),
}));

function userMessage(text: string): UIMessage {
  return { id: 'u1', role: 'user', parts: [{ type: 'text', text }] };
}

function sendOpts(messages: UIMessage[]) {
  return {
    trigger: 'submit-message' as const,
    chatId: 'conv-1',
    messageId: undefined,
    messages,
    abortSignal: undefined,
  };
}

describe('VanylineChatTransport', () => {
  beforeEach(() => {
    wsInstances.length = 0;
  });

  it('envoie le texte du dernier message user sur le WS', async () => {
    const transport = new VanylineChatTransport();
    await transport.sendMessages(sendOpts([userMessage('salut')]));

    expect(wsInstances[0].sent).toEqual([JSON.stringify({ type: 'message', content: 'salut' })]);
  });

  it('token(s) puis done -> text-start/text-delta*/text-end/finish', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit('message', JSON.stringify({ type: 'token', content: 'Bon' }));
    await readOne(); // text-start
    await readOne(); // text-delta "Bon"
    ws.emit('message', JSON.stringify({ type: 'token', content: 'jour' }));
    await readOne(); // text-delta "jour"
    ws.emit('message', JSON.stringify({ type: 'done' }));
    await readOne(); // text-end
    await readOne(); // finish

    expect(chunks[0]).toEqual({ type: 'start' });
    expect(chunks[1]).toMatchObject({ type: 'text-start' });
    const textId = (chunks[1] as { id: string }).id;
    expect(chunks[2]).toEqual({ type: 'text-delta', id: textId, delta: 'Bon' });
    expect(chunks[3]).toEqual({ type: 'text-delta', id: textId, delta: 'jour' });
    expect(chunks[4]).toEqual({ type: 'text-end', id: textId });
    expect(chunks[5]).toEqual({ type: 'finish' });
    expect(ws.close).toHaveBeenCalled();
  });

  it('reasoning_delta(s) puis token -> reasoning-start/delta*/end avant le text-start', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit('message', JSON.stringify({ type: 'reasoning_delta', content: 'je ré' }));
    await readOne(); // reasoning-start
    await readOne(); // reasoning-delta "je ré"
    ws.emit('message', JSON.stringify({ type: 'reasoning_delta', content: 'fléchis' }));
    await readOne(); // reasoning-delta "fléchis"
    ws.emit('message', JSON.stringify({ type: 'token', content: 'Voici' }));
    await readOne(); // reasoning-end (fermé par le premier token)
    await readOne(); // text-start
    await readOne(); // text-delta "Voici"

    expect(chunks[1]).toMatchObject({ type: 'reasoning-start' });
    const reasoningId = (chunks[1] as { id: string }).id;
    expect(chunks[2]).toEqual({ type: 'reasoning-delta', id: reasoningId, delta: 'je ré' });
    expect(chunks[3]).toEqual({ type: 'reasoning-delta', id: reasoningId, delta: 'fléchis' });
    expect(chunks[4]).toEqual({ type: 'reasoning-end', id: reasoningId });
    expect(chunks[5]).toMatchObject({ type: 'text-start' });
  });

  it('deux segments de texte séparés par un tool_call restent deux parts distinctes, dans l\'ordre chronologique', async () => {
    // Régression : reasoning -> "je cherche..." (texte) -> find_file -> reasoning ->
    // "voici le résumé" (texte) -> done. Avant le fix, les deux segments de texte
    // étaient fusionnés dans la même part `text` (même id jamais refermé au
    // tool_call), ce qui faisait apparaître tout le texte à la position du
    // PREMIER segment, avant les tool calls survenus entre les deux.
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('résume le README')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit('message', JSON.stringify({ type: 'token', content: 'je cherche...' }));
    await readOne(); // text-start (id A)
    await readOne(); // text-delta A
    ws.emit(
      'message',
      JSON.stringify({ type: 'tool_call', id: 't1', name: 'find_file', args: {} }),
    );
    await readOne(); // text-end (id A) — le tool_call doit fermer le bloc texte ouvert
    await readOne(); // tool-input-available
    ws.emit(
      'message',
      JSON.stringify({ type: 'tool_result', id: 't1', name: 'find_file', result: 'README.md', is_error: false }),
    );
    await readOne(); // tool-output-available
    ws.emit('message', JSON.stringify({ type: 'token', content: 'voici le résumé' }));
    await readOne(); // text-start (id B, différent de A)
    await readOne(); // text-delta B
    ws.emit('message', JSON.stringify({ type: 'done' }));
    await readOne(); // text-end (id B)
    await readOne(); // finish

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

  it('reasoning_delta seul (pas de texte) -> reasoning-end émis sur done', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit('message', JSON.stringify({ type: 'reasoning_delta', content: 'hmm' }));
    await readOne(); // reasoning-start
    await readOne(); // reasoning-delta
    ws.emit('message', JSON.stringify({ type: 'done' }));
    await readOne(); // reasoning-end
    await readOne(); // finish

    expect(chunks[3]).toMatchObject({ type: 'reasoning-end' });
    expect(chunks[4]).toEqual({ type: 'finish' });
  });

  it('tool_call -> tool-input-available (dynamic:true), tool_result -> tool-output-available', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit(
      'message',
      JSON.stringify({ type: 'tool_call', id: 't1', name: 'read_file', args: { path: 'a' } }),
    );
    await readOne();
    ws.emit(
      'message',
      JSON.stringify({ type: 'tool_result', id: 't1', name: 'read_file', result: 'contenu', is_error: false }),
    );
    await readOne();

    expect(chunks[1]).toEqual({
      type: 'tool-input-available',
      toolCallId: 't1',
      toolName: 'read_file',
      input: { path: 'a' },
      dynamic: true,
    });
    expect(chunks[2]).toEqual({ type: 'tool-output-available', toolCallId: 't1', output: 'contenu' });
  });

  it('tool_unavailable -> chunk data-tool_unavailable, tour non interrompu', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit(
      'message',
      JSON.stringify({ type: 'tool_unavailable', server: 'sandbox', reason: 'sandbox introuvable' }),
    );
    await readOne();

    expect(chunks[1]).toMatchObject({
      type: 'data-tool_unavailable',
      data: { server: 'sandbox', reason: 'sandbox introuvable' },
    });
    expect(ws.close).not.toHaveBeenCalled();
  });

  it('error -> chunk error puis fermeture (pas de finish)', async () => {
    const transport = new VanylineChatTransport();
    const stream = await transport.sendMessages(sendOpts([userMessage('salut')]));
    const ws = wsInstances[0];
    const reader = stream.getReader();
    const chunks: unknown[] = [];
    const readOne = async () => chunks.push((await reader.read()).value);

    await readOne(); // start
    ws.emit('message', JSON.stringify({ type: 'error', code: 'VNL-LLM-001', message: 'boom' }));
    await readOne();

    expect(chunks[1]).toEqual({ type: 'error', errorText: 'boom' });
    expect(ws.close).toHaveBeenCalled();
    const { done } = await reader.read();
    expect(done).toBe(true);
  });

  it('reconnectToStream retourne toujours null (pas de reprise de flux)', async () => {
    const transport = new VanylineChatTransport();
    await expect(
      transport.reconnectToStream({ chatId: 'conv-1' }),
    ).resolves.toBeNull();
  });
});
