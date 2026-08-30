import { beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import ChatWindow from './ChatWindow.vue';
import type { ChatBackend, ChatTransport, UIMessage } from '../ports';

function createBackend(overrides: Partial<ChatBackend> = {}): ChatBackend {
  return {
    listConversations: vi.fn(async () => [
      { id: 'conv-1', title: 'Test 1', createdAt: '2025-01-01T00:00:00Z' },
      { id: 'conv-2', title: null, createdAt: '2025-02-01T00:00:00Z' },
    ]),
    loadMessages: vi.fn(async () => []),
    createConversation: vi.fn(async () => 'conv-new'),
    ...overrides,
  } as unknown as ChatBackend;
}

function createTransport(): ChatTransport<UIMessage> {
  return {
    sendMessages: vi.fn(),
    reconnectToStream: vi.fn(async () => null),
  } as unknown as ChatTransport<UIMessage>;
}

const backend = createBackend();
const transport = createTransport();

function host(extraProps: Record<string, unknown> = {}) {
  return mount(ChatWindow, {
    props: { activeConversationId: null, ...extraProps },
    global: {
      provide: {
        'vanyline.chatBackend': backend,
        'vanyline.chatTransport': transport,
      },
    },
  });
}

async function tick() {
  await nextTick();
}

describe('ChatWindow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('liste les conversations au montage', async () => {
    const wrapper = host();
    await tick();

    expect(backend.listConversations).toHaveBeenCalledTimes(1);
    const select = wrapper.find('.session-select');
    expect(select.find('option').exists()).toBe(true);
    expect(wrapper.text()).toContain('Test 1');
    // conv-2 a title=null → label formé date
    expect(wrapper.text()).toContain('Session du');
    wrapper.unmount();
  });

  it('charge l\'historique quand une conversation est active', async () => {
    const wrapper = host({ activeConversationId: 'conv-1' });
    await tick();

    expect(backend.loadMessages).toHaveBeenCalledWith('conv-1');
    wrapper.unmount();
  });

  it('le sélecteur change → émet update:activeConversationId', async () => {
    const wrapper = host({ activeConversationId: null });
    await tick();

    const select = wrapper.find('.session-select');
    select.setValue('conv-1');
    await tick();

    const emitted = wrapper.emitted('update:activeConversationId');
    expect(emitted ?? []).toHaveLength(1);
    expect((emitted![0] as string[])[0]).toBe('conv-1');
    wrapper.unmount();
  });

  it('bouton × → émet update:activeConversationId null', async () => {
    const wrapper = host({ activeConversationId: 'conv-1' });
    await tick();

    wrapper.find('[title="Fermer la session"]').trigger('click');
    await tick();

    const emitted = wrapper.emitted('update:activeConversationId');
    expect(emitted).toHaveLength(1);
    expect((emitted![0] as string[])[0]).toBeNull();
    wrapper.unmount();
  });

  it('bouton + → createConversation puis émet l\'id', async () => {
    vi.mocked(backend.createConversation).mockResolvedValue('new-42');
    const wrapper = host();
    await tick();

    wrapper.find('[title="Nouvelle session"]').trigger('click');
    await tick();

    expect(backend.createConversation).toHaveBeenCalledTimes(1);
    const emitted = wrapper.emitted('update:activeConversationId');
    expect(emitted).toHaveLength(1);
    expect((emitted![0] as string[])[0]).toBe('new-42');
    wrapper.unmount();
  });

  it('startingSession=true → bouton + disabled', async () => {
    const wrapper = host({ startingSession: true });
    await tick();

    const btn = wrapper.find('[title="Nouvelle session"]');
    expect(btn.attributes('disabled')).toBeDefined();
    wrapper.unmount();
  });

  it('échec createConversation ignoré (catch silencieux)', async () => {
    vi.mocked(backend.createConversation).mockRejectedValue(new Error('fail'));

    let errorCaught = false;
    const original = console.error;
    console.error = (...args: unknown[]) => { errorCaught = true; original(...args); };

    const wrapper = host();
    await tick();

    wrapper.find('[title="Nouvelle session"]').trigger('click');
    await tick();

    const emitted = wrapper.emitted('update:activeConversationId');
    expect(emitted).toBeUndefined();
    expect(errorCaught).toBe(false);

    console.error = original;
    wrapper.unmount();
  });
});