import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ChatBackend } from '@vanyline/ui';
import Chat from './Chat.vue';
import { clearIdeActions, useIdeSession } from '../../composables/useIdeSession';
import { httpChatBackend } from '../../api/httpChatBackend';

// La sandbox courante est fournie par `IdeShell.vue` via provide/inject
// (même pattern qu'Explorer.vue) — sans elle Chat.vue ne peut ni filtrer
// la liste des conversations ni poser le contexte à la création.
const provideSandboxName = { 'sandbox-name': 'my-sandbox' };

function router() {
  return createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: {} }] });
}

const { wsInstances } = vi.hoisted(() => ({
  wsInstances: [] as Array<{
    listeners: Record<string, Array<(ev: { data?: unknown }) => void>>;
    sent: string[];
    close: () => void;
    send: (data: string) => void;
    emit: (type: string, data?: unknown) => void;
  }>,
}));

vi.mock('../../api/chatWs', () => ({
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

const mockBackend = vi.mocked<ChatBackend>({
  listConversations: vi.fn(async () => []),
  loadMessages: vi.fn(async () => []),
  createConversation: vi.fn(async () => '42'),
});

vi.mock('../../api/httpChatBackend', () => ({
  httpChatBackend: vi.fn(() => mockBackend),
}));

const { fetchSpy } = vi.hoisted(() => ({ fetchSpy: vi.fn() }));
vi.stubGlobal('fetch', fetchSpy);

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
  );
}

/** Route par URL plutôt que par ordre d'appel. `messagesByConv` : historique
 *  par id de conversation, vide par défaut. */
function mockFetchRouting(messagesByConv: Record<string, unknown[]> = {}) {
  fetchSpy.mockImplementation((url: string) => {
    if (url.startsWith('/api/conversations?')) return jsonResponse([]);
    const match = url.match(/^\/api\/conversations\/([^/]+)\/messages$/);
    if (match) return jsonResponse(messagesByConv[match[1]] ?? []);
    return jsonResponse([]);
  });
}

/** Macrotask flush plutôt qu'un nombre fixe de microtasks — cf. la même
 *  leçon dans sandboxWs.spec.ts / l'ancien Chat.spec.ts. */
async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('Chat.vue — session réelle', () => {
  beforeEach(() => {
    wsInstances.length = 0;
    fetchSpy.mockReset();
    mockBackend.listConversations.mockReset();
    mockBackend.loadMessages.mockReset();
    mockBackend.createConversation.mockReset();
    vi.mocked(httpChatBackend).mockClear();
    clearIdeActions();
  });

  afterEach(() => {
    clearIdeActions();
  });

  it("charge l'historique quand une conversation est active", async () => {
    mockFetchRouting({
      'conv-1': [
        {
          id: 1,
          role: 'user',
          payload: { content: 'salut' },
          created_at: '2026-01-01T10:00:00Z',
        },
      ],
    });

    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-1';

    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();
    await flush();

    expect(mockBackend.loadMessages).toHaveBeenCalledWith('conv-1');
    // Contrairement à l'ancien vue-advanced-chat, le WS n'est plus ouvert
    // par avance à l'activation de la conversation — le transport AI SDK
    // (`VanylineChatTransport`) n'ouvre une connexion qu'au moment d'un
    // envoi réel (cf. le test "envoie ... sur le WS" ci-dessous). Le backend
    // ne pousse rien sans un message entrant, donc rien n'est perdu.
    expect(wsInstances.length).toBe(0);
    wrapper.unmount();
  });

  it('liste les conversations filtrées par sandbox_name', async () => {
    mockFetchRouting();
    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();

    // Le wrapper injecte sandbox-name et construit httpChatBackend avec elle :
    // la liste est scopée à la sandbox courante.
    expect(vi.mocked(httpChatBackend)).toHaveBeenCalledWith('my-sandbox');
    expect(mockBackend.listConversations).toHaveBeenCalled();
    wrapper.unmount();
  });

  it("soumettre le prompt ouvre le WS et envoie {type:'message', content}", async () => {
    mockFetchRouting();
    mockBackend.listConversations.mockResolvedValueOnce([]);
    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-3';

    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();
    await flush();

    const textarea = wrapper.find('textarea');
    expect(textarea.exists()).toBe(true);
    await textarea.setValue('hello agent');
    await wrapper.find('form').trigger('submit');
    await flush();

    expect(wsInstances.length).toBe(1);
    expect(wsInstances[0].sent).toEqual([
      JSON.stringify({ type: 'message', content: 'hello agent' }),
    ]);

    // Le flux d'événements jusqu'au bout (token/done) est couvert en détail
    // par chatTransport.spec.ts — ici on vérifie seulement que ça ne casse
    // pas la stack Chat.vue/ChatSession/AI SDK.
    wsInstances[0].emit('message', JSON.stringify({ type: 'token', content: 'salut' }));
    wsInstances[0].emit('message', JSON.stringify({ type: 'done' }));
    await flush();

    wrapper.unmount();
  });

  it('le sélecteur de session liste les conversations et change activeConversationId', async () => {
    mockBackend.listConversations.mockResolvedValueOnce([
      { id: '1', title: 'Session A', createdAt: '2026-01-01T10:00:00Z' },
      { id: '2', title: 'Session B', createdAt: '2026-01-02T10:00:00Z' },
    ] as import('@vanyline/ui').ChatConversation[]);

    const { activeConversationId } = useIdeSession();
    activeConversationId.value = '1';

    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();
    await flush();

    const options = wrapper.findAll('option');
    expect(options.map((o) => o.text())).toEqual(['Session A', 'Session B']);

    await wrapper.find('.session-select').setValue('2');

    expect(activeConversationId.value).toBe('2');
    wrapper.unmount();
  });

  it('"Fermer la session" désactive activeConversationId', async () => {
    mockFetchRouting();
    const { activeConversationId } = useIdeSession();
    activeConversationId.value = 'conv-x';

    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();
    await flush();

    await wrapper.find('.session-btn[title="Fermer la session"]').trigger('click');

    expect(activeConversationId.value).toBeNull();
    wrapper.unmount();
  });

  it('"Nouvelle session" appelle httpChatBackend.createConversation', async () => {
    mockFetchRouting();
    mockBackend.createConversation.mockClear();

    const wrapper = mount(Chat, { global: { plugins: [router()], provide: provideSandboxName } });
    await flush();

    await wrapper.find('.session-btn[title="Nouvelle session"]').trigger('click');
    await flush();

    expect(mockBackend.createConversation).toHaveBeenCalledTimes(1);
    expect(useIdeSession().activeConversationId.value).toBe('42');
    wrapper.unmount();
  });
});