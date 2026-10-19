import { describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import { useChat } from '@ai-sdk/vue';
import type { UIMessage } from '../ports';
import ChatSession from './ChatSession.vue';

// La stack AI SDK est mockée au point `useChat` : on pilote `messages`
// directement depuis les tests (le flux transport/streaming complet est
// couvert par ChatWindow.spec.ts — ici seul le watch
// FS_MUTATING_TOOLS → notify-fs-change nous intéresse, cf. tâche 08d de
// `lsp-agent-interface` qui y ajoute `edit_and_check`).
vi.mock('@ai-sdk/vue', async () => {
  const { ref } = await import('vue');
  return {
    useChat: vi.fn(() => ({
      messages: ref([]),
      status: ref('ready'),
      sendMessage: vi.fn(),
      error: ref(null),
      stop: vi.fn(),
    })),
  };
});

function toolMessage(toolName: string): UIMessage {
  return {
    id: `msg-${toolName}`,
    role: 'assistant',
    parts: [
      {
        type: 'dynamic-tool',
        toolName,
        toolCallId: `call-${toolName}`,
        state: 'output-available',
        input: {},
        output: 'ok',
      },
    ],
  } as unknown as UIMessage;
}

function mountSession() {
  const notifyFsChange = vi.fn();
  const wrapper = mount(ChatSession, {
    props: { conversationId: 'conv-1' },
    global: {
      provide: {
        'vanyline.chatBackend': {
          listConversations: vi.fn(async () => []),
          loadMessages: vi.fn(async () => []),
          createConversation: vi.fn(async () => 'conv-1'),
        },
        'vanyline.chatTransport': {
          sendMessages: vi.fn(),
          reconnectToStream: vi.fn(async () => null),
        },
        'notify-fs-change': notifyFsChange,
      },
      stubs: {
        UChatMessages: true,
        UChatReasoning: true,
        UChatTool: true,
        UChatPrompt: true,
        UChatPromptSubmit: true,
        Markdown: true,
      },
    },
  });
  return { wrapper, notifyFsChange };
}

function latestMessages() {
  const call = vi.mocked(useChat).mock.results.at(-1);
  if (!call) throw new Error('useChat should have been called by ChatSession setup');
  return call.value.messages;
}

async function flush() {
  await nextTick();
  await nextTick();
}

describe('ChatSession — FS_MUTATING_TOOLS → notify-fs-change', () => {
  it('edit_and_check (tâche 08d) déclenche le refresh FS comme les autres outils mutateurs', async () => {
    const { wrapper, notifyFsChange } = mountSession();
    latestMessages().value = [toolMessage('edit_and_check')];
    await flush();
    expect(notifyFsChange).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it('les outils mutateurs historiques déclenchent toujours le refresh', async () => {
    const { wrapper, notifyFsChange } = mountSession();
    for (const toolName of ['write_file', 'edit_file', 'delete_file', 'execute_command']) {
      latestMessages().value = [toolMessage(toolName)];
      await flush();
    }
    expect(notifyFsChange).toHaveBeenCalledTimes(4);
    wrapper.unmount();
  });

  it('un outil en lecture seule ne déclenche pas le refresh', async () => {
    const { wrapper, notifyFsChange } = mountSession();
    latestMessages().value = [toolMessage('read_file')];
    await flush();
    expect(notifyFsChange).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});

// Scenario `champ_de_saisie_compact` (chat.sdd) : champ d'une ligne au repos, paddings
// réduits par l'emplacement `ui`, bouton d'envoi petit (xs/sm). Les composants Nuxt UI sont
// auto-importés : on inspecte les vrais composants (pas de stub, qui ne s'applique pas ici).
describe('ChatSession — champ de saisie compact (champ_de_saisie_compact)', () => {
  function mountSessionReal() {
    return mount(ChatSession, {
      props: { conversationId: 'conv-1' },
      global: {
        provide: {
          'vanyline.chatBackend': {
            listConversations: vi.fn(async () => []),
            loadMessages: vi.fn(async () => []),
            createConversation: vi.fn(async () => 'conv-1'),
          },
          'vanyline.chatTransport': {
            sendMessages: vi.fn(),
            reconnectToStream: vi.fn(async () => null),
          },
          'notify-fs-change': vi.fn(),
        },
        stubs: { UChatMessages: true, UChatReasoning: true, UChatTool: true, Markdown: true },
      },
    });
  }

  it('UChatPrompt : une ligne au repos, paddings réduits via `ui`, croissance plafonnée à 6 lignes', () => {
    const wrapper = mountSessionReal();
    const prompt = wrapper.findComponent({ name: 'ChatPrompt' });
    expect(prompt.exists()).toBe(true);
    expect(prompt.props('rows')).toBe(1);
    expect(prompt.props('maxrows')).toBe(6);
    const ui = prompt.props('ui') as Record<string, string> | undefined;
    expect(ui).toBeDefined();
    expect(ui?.root).toMatch(/(?:^|\s)py-1(?:\s|$)/);
    expect(ui?.root).not.toMatch(/(?:^|\s)py-2(?:\s|$)/);
    expect(ui?.base).toMatch(/(?:^|\s)py-1(?:\s|$)/);
    wrapper.unmount();
  });

  it('UChatPromptSubmit : bouton petit (xs ou sm), jamais lg', () => {
    const wrapper = mountSessionReal();
    const submit = wrapper.findComponent({ name: 'ChatPromptSubmit' });
    expect(submit.exists()).toBe(true);
    expect(['xs', 'sm']).toContain(submit.props('size'));
    wrapper.unmount();
  });
});
