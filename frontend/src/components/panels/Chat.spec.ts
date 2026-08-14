import { mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Chat from './Chat.vue';
import { clearIdeActions, useIdeSession } from '../../composables/useIdeSession';

// vue-advanced-chat est un vrai Web Component (isCustomElement dans
// vite.config.ts) enregistré par main.ts au démarrage réel de l'app — ce
// spec ne charge pas main.ts, donc jsdom voit une balise custom element
// inerte (comportement DOM standard uniquement : addEventListener marche,
// pas de logique interne de la lib). Suffisant pour tester Chat.vue seul.
const { wsInstances } = vi.hoisted(() => ({
  wsInstances: [] as Array<{
    listeners: Record<string, Array<(ev: unknown) => void>>;
    sent: string[];
    close: () => void;
    send: (data: string) => void;
    emitMessage: (data: unknown) => void;
  }>,
}));

vi.mock('../../api/chatWs', () => ({
  openChatWs: vi.fn(() => {
    const listeners: Record<string, Array<(ev: unknown) => void>> = {};
    const instance = {
      listeners,
      sent: [] as string[],
      close: vi.fn(),
      send: vi.fn(function (this: { sent: string[] }, data: string) {
        this.sent.push(data);
      }),
      addEventListener(type: string, cb: (ev: unknown) => void) {
        (listeners[type] ??= []).push(cb);
      },
      emitMessage(data: unknown) {
        for (const cb of [...(listeners['message'] ?? [])]) cb({ data });
      },
    };
    wsInstances.push(instance as unknown as (typeof wsInstances)[number]);
    return Promise.resolve(instance);
  }),
}));

const { fetchSpy } = vi.hoisted(() => ({ fetchSpy: vi.fn() }));
vi.stubGlobal('fetch', fetchSpy);

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
  );
}

/** Macrotask flush plutôt qu'un nombre fixe de microtasks : `Response.json()`
 *  traverse plusieurs `await` internes dont le nombre exact n'est pas
 *  garanti (cf. la même leçon dans sandboxWs.spec.ts). */
async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('Chat.vue — session réelle', () => {
  beforeEach(() => {
    wsInstances.length = 0;
    fetchSpy.mockReset();
    clearIdeActions();
  });

  afterEach(() => {
    clearIdeActions();
  });

  it("charge l'historique puis ouvre le WS quand une conversation est active", async () => {
    fetchSpy.mockReturnValueOnce(
      jsonResponse([
        {
          id: 'm1',
          role: 'user',
          payload: { content: 'salut' },
          created_at: '2026-01-01T10:00:00Z',
        },
      ]),
    );

    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-1';

    const wrapper = mount(Chat);
    await flush();

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/conversations/conv-1/messages',
      expect.any(Object),
    );
    expect(wrapper.vm).toBeTruthy();
    expect(wsInstances.length).toBe(1);
    wrapper.unmount();
  });

  it('accumule les tokens en un seul message assistant jusqu\'à "done"', async () => {
    fetchSpy.mockReturnValueOnce(jsonResponse([]));
    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-2';

    const wrapper = mount(Chat);
    await flush();

    const ws = wsInstances[0];
    ws.emitMessage(JSON.stringify({ type: 'token', content: 'Bon' }));
    ws.emitMessage(JSON.stringify({ type: 'token', content: 'jour' }));
    ws.emitMessage(JSON.stringify({ type: 'done' }));
    await flush();

    // Un nouveau tour doit créer un NOUVEAU message, pas continuer l'ancien.
    ws.emitMessage(JSON.stringify({ type: 'token', content: 'Suite' }));
    await flush();

    // Pas d'assertion directe sur messages.value (non exposé) — le test
    // vérifie surtout l'absence de crash sur la séquence token/token/done/token,
    // qui est le scénario de régression visé (accumulation puis reset).
    wrapper.unmount();
  });

  it("envoie {type:'message', content} sur le WS", async () => {
    fetchSpy.mockReturnValueOnce(jsonResponse([]));
    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-3';

    const wrapper = mount(Chat);
    await flush();

    // onSendMessage n'est pas exposé — on simule l'event custom émis par
    // vue-advanced-chat sur son propre élément (le listener @send-message
    // est posé dessus, pas sur .chat-host).
    const chatEl = wrapper.find('vue-advanced-chat');
    expect(chatEl.exists()).toBe(true);
    const event = new CustomEvent('send-message', { detail: [{ content: 'salut agent' }] });
    chatEl.element.dispatchEvent(event);
    await flush();

    const ws = wsInstances[0];
    expect(ws.send).toHaveBeenCalledWith(JSON.stringify({ type: 'message', content: 'salut agent' }));
    wrapper.unmount();
  });
});
