import { describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';
import { mount } from '@vue/test-utils';
import { useChat } from '@ai-sdk/vue';
import type { UIMessage } from 'ai';
import ChatSession from './ChatSession.vue';

// Même harness que `packages/ui/src/chat/ChatSession.spec.ts` (copie
// historique de ce composant, avec transport/backend câblés en dur) : la
// stack AI SDK est mockée au point `useChat` pour piloter `messages`
// directement — seul le watch FS_MUTATING_TOOLS → notify-fs-change est
// testé ici (tâche 08d de `lsp-agent-interface`, qui ajoute
// `edit_and_check`). Les deux copies doivent rester synchrones.
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

vi.mock('../../api/chatTransport', () => ({
  VanylineChatTransport: class {
    sendMessages = vi.fn();
    reconnectToStream = vi.fn(async () => null);
  },
}));

vi.mock('../../api/client', () => ({
  createApiClient: () => ({
    get: vi.fn(async () => []),
    post: vi.fn(async () => ({})),
  }),
}));

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

describe('ChatSession (copie frontend) — FS_MUTATING_TOOLS → notify-fs-change', () => {
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
