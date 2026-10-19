import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { ChatWindow } from '@vanyline/ui';
import ContextChat from './ContextChat.vue';

const { wsInstances } = vi.hoisted(() => ({
  wsInstances: [] as Array<{
    listeners: Record<string, Array<(ev: { data?: unknown }) => void>>;
    sent: string[];
    close: () => void;
    send: (data: string) => void;
    emit: (type: string, data?: unknown) => void;
  }>,
}));

vi.mock('../api/chatWs', () => ({
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

const { fetchSpy } = vi.hoisted(() => ({ fetchSpy: vi.fn() }));
vi.stubGlobal('fetch', fetchSpy);

// Éléments de scénario ---------------------------------------------------

interface Call {
  method: string;
  url: string;
  body?: Record<string, unknown>;
}
type Responder = (call: Call) => Response | undefined;

const calls: Call[] = [];
let routes: Responder[] = [];

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

/** Route ajoutée en tête : la dernière déclarée l'emporte sur les défauts. */
function on(method: string, url: string, respond: (call: Call) => Response) {
  routes.unshift((call) => (call.method === method && call.url === url ? respond(call) : undefined));
}

function calledWith(method: string, url: string): Call[] {
  return calls.filter((c) => c.method === method && c.url === url);
}

/** Laisse passer les promesses et les macrotâches (fetch mocké, WS mocké). */
async function settle() {
  for (let i = 0; i < 6; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

function convOut(id: number, agentName: string | null, kind: string, data: Record<string, unknown>) {
  return {
    id,
    agent_name: agentName,
    context: { id: id * 10, kind, data, created_at: '2026-10-01T10:00:00Z' },
    title: null,
    todo: null,
    created_at: '2026-10-01T10:00:00Z',
    updated_at: '2026-10-01T10:00:00Z',
  };
}

const CONFIG_ROW = {
  id: 10,
  owner_id: 1,
  name: 'system-config',
  description: 'Agent de configuration',
  mode: 'primary',
  model_profile_id: 1,
  toolsets: [],
  skills: 'auto',
  system_prompt: 'PROMPT SYSTÈME',
  system_key: 'config',
  system_version: 2,
};

beforeEach(() => {
  calls.length = 0;
  wsInstances.length = 0;
  routes = [];
  fetchSpy.mockReset();
  fetchSpy.mockImplementation(async (input: unknown, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? 'GET';
    const body = init?.body ? (JSON.parse(init.body as string) as Record<string, unknown>) : undefined;
    const call: Call = { method, url, body };
    calls.push(call);
    for (const route of routes) {
      const res = route(call);
      if (res) return res;
    }
    // Défauts : listes vides, historique vide. Toute autre URL échoue bruyamment.
    if (method === 'GET' && url === '/api/conversations') return json([]);
    if (method === 'GET' && url === '/api/v1/agents') return json([]);
    if (method === 'GET' && url === '/api/v1/model-profiles') return json([]);
    if (method === 'GET' && /^\/api\/conversations\/[^/]+\/messages$/.test(url)) return json([]);
    return json({ error: `route non prévue : ${method} ${url}` }, 500);
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

// Tests ------------------------------------------------------------------

describe('ContextChat', () => {
  it('creation_de_la_conversation_du_contexte', async () => {
    on('POST', '/api/conversations', () =>
      json(convOut(5, 'system-project', 'project', { project_name: 'demo' }), 201),
    );

    mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    const posts = calledWith('POST', '/api/conversations');
    expect(posts).toHaveLength(1);
    expect(posts[0].body).toEqual({
      agent_name: 'system-project',
      context: { kind: 'project', data: { project_name: 'demo' } },
    });
  });

  it('reutilisation_de_la_conversation', async () => {
    on('GET', '/api/conversations', () =>
      json([
        convOut(8, 'system-project', 'project', { project_name: 'autre' }),
        convOut(7, 'system-project', 'project', { project_name: 'demo' }),
      ]),
    );

    mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    expect(calledWith('POST', '/api/conversations')).toHaveLength(0);
    expect(calledWith('GET', '/api/conversations/7/messages')).toHaveLength(1);
    expect(calledWith('GET', '/api/conversations/8/messages')).toHaveLength(0);
  });

  it('settings_choix_du_modele_seul', async () => {
    on('GET', '/api/conversations', () => json([convOut(3, 'system-config', 'settings', {})]));
    on('GET', '/api/v1/agents', () => json([CONFIG_ROW]));
    on('GET', '/api/v1/model-profiles', () =>
      json([
        { id: 1, name: 'petit', provider_id: 4, model: 'm1' },
        { id: 2, name: 'grand', provider_id: 4, model: 'm2' },
      ]),
    );
    on('PUT', '/api/v1/agents/10', (call) => json({ ...CONFIG_ROW, ...call.body }));

    const wrapper = mount(ContextChat, { props: { kind: 'settings', data: {} } });
    await settle();

    expect(wrapper.find('select[aria-label="Agent"]').exists()).toBe(false);
    const modelSelect = wrapper.find('select[aria-label="Modèle de l\'assistant"]');
    expect(modelSelect.exists()).toBe(true);

    await modelSelect.setValue('2');
    await settle();

    const puts = calledWith('PUT', '/api/v1/agents/10');
    expect(puts).toHaveLength(1);
    expect(puts[0].body).toEqual({ ...CONFIG_ROW, model_profile_id: 2 });
    expect(calls.filter((c) => c.method === 'PUT')).toHaveLength(1);
  });

  it('choix_d_un_agent_ordinaire', async () => {
    const sandboxData = { sandbox_name: 's1' };
    on('POST', '/api/conversations', (call) =>
      json(
        convOut(call.body?.agent_name === 'alpha' ? 11 : 9, call.body?.agent_name as string, 'sandbox', sandboxData),
        201,
      ),
    );
    on('GET', '/api/v1/agents', () =>
      json([
        { ...CONFIG_ROW, id: 20, name: 'system-sandbox', system_key: 'sandbox' },
        { ...CONFIG_ROW, id: 21, name: 'alpha', system_key: null },
        { ...CONFIG_ROW, id: 22, name: 'beta', system_key: null },
      ]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'sandbox', data: sandboxData } });
    await settle();

    expect(calledWith('POST', '/api/conversations')[0].body?.agent_name).toBe('system-sandbox');

    const options = wrapper.findAll('select[aria-label="Agent"] option').map((o) => (o.element as HTMLOptionElement).value);
    expect(options).toEqual(['system-sandbox', 'alpha', 'beta']);

    await wrapper.find('select[aria-label="Agent"]').setValue('alpha');
    await settle();

    const posts = calledWith('POST', '/api/conversations');
    expect(posts).toHaveLength(2);
    expect(posts[1].body).toEqual({ agent_name: 'alpha', context: { kind: 'sandbox', data: sandboxData } });
    expect(calledWith('GET', '/api/conversations/11/messages')).toHaveLength(1);
  });

  it('aucun_profil_de_modele', async () => {
    on('POST', '/api/conversations', () => json({ error: 'VNL-LLM-008: No model configured' }, 422));

    const wrapper = mount(ContextChat, { props: { kind: 'settings', data: {} } });
    await settle();

    const state = wrapper.find('.context-chat__no-profile');
    expect(state.exists()).toBe(true);
    expect(state.text()).toContain('Aucun profil de modèle');
    expect(wrapper.findComponent(ChatWindow).exists()).toBe(false);
    expect(wrapper.find('textarea').exists()).toBe(false);
  });

  it('fin_de_tour_notifie_l_appelant', async () => {
    on('GET', '/api/conversations', () =>
      json([convOut(5, 'system-project', 'project', { project_name: 'demo' })]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    await wrapper.find('textarea').setValue('bonjour');
    await wrapper.find('form').trigger('submit');
    await settle();

    const ws = wsInstances[0];
    expect(ws).toBeDefined();
    ws.emit('message', JSON.stringify({ type: 'token', content: 'salut' }));
    ws.emit('message', JSON.stringify({ type: 'done' }));
    await settle();

    expect(wrapper.emitted('turn-finished')).toHaveLength(1);
  });

  it('erreur_d_un_tour', async () => {
    on('GET', '/api/conversations', () =>
      json([convOut(5, 'system-project', 'project', { project_name: 'demo' })]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    await wrapper.find('textarea').setValue('bonjour');
    await wrapper.find('form').trigger('submit');
    await settle();

    const ws = wsInstances[0];
    ws.emit('message', JSON.stringify({ type: 'error', message: 'fournisseur injoignable' }));
    await settle();

    expect(wrapper.find('.chat-error').text()).toContain('fournisseur injoignable');
    expect(wrapper.find('.context-chat__no-profile').exists()).toBe(false);
    const textarea = wrapper.find('textarea').element as HTMLTextAreaElement;
    expect(textarea.disabled).toBe(false);
    expect(wrapper.emitted('turn-finished')).toHaveLength(1);
  });

  it('erreur_de_tour_sans_profil_passe_en_etat_explicite', async () => {
    on('GET', '/api/conversations', () =>
      json([convOut(5, 'system-project', 'project', { project_name: 'demo' })]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    await wrapper.find('textarea').setValue('bonjour');
    await wrapper.find('form').trigger('submit');
    await settle();

    wsInstances[0].emit('message', JSON.stringify({ type: 'error', message: 'VNL-LLM-008: No model configured' }));
    await settle();

    expect(wrapper.find('.context-chat__no-profile').exists()).toBe(true);
    expect(wrapper.find('textarea').exists()).toBe(false);
  });

  it('aucun_secret_affiche', async () => {
    on('GET', '/api/conversations', () => json([convOut(3, 'system-config', 'settings', {})]));
    on('GET', '/api/v1/agents', () => json([CONFIG_ROW]));
    on('GET', '/api/v1/model-profiles', () =>
      json([{ id: 1, name: 'petit', provider_id: 4, model: 'm1', api_key: 'sk-test-SECRET-123' }]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'settings', data: {} } });
    await settle();

    // Garde anti-vacuité : le contenu sensible est bien chargé et le chat rendu.
    expect(calledWith('GET', '/api/v1/model-profiles')).toHaveLength(1);
    expect(wrapper.findComponent(ChatWindow).exists()).toBe(true);
    expect(wrapper.html()).not.toContain('sk-test-SECRET-123');
    expect(wrapper.text()).not.toContain('sk-test-SECRET-123');
  });

  it('feuille_plein_ecran_sous_md', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 360 });
    on('GET', '/api/conversations', () =>
      json([convOut(5, 'system-project', 'project', { project_name: 'demo' })]),
    );

    const wrapper = mount(ContextChat, { props: { kind: 'project', data: { project_name: 'demo' } } });
    await settle();

    // jsdom ne calcule pas la mise en page : on vérifie que la racine occupe
    // son conteneur sans largeur fixe imposée en ligne. Le rendu réel à 360 px
    // reste à vérifier visuellement.
    const root = wrapper.find('.context-chat');
    expect(root.exists()).toBe(true);
    expect(root.attributes('style')).toBeUndefined();
    // Garde anti-vacuité : le chat est bien rendu dans la racine.
    expect(wrapper.findComponent(ChatWindow).exists()).toBe(true);
  });
});
