import { listConversations, createConversation, deleteConversation } from '$lib/api/conversations';
import type { Conversation } from '$lib/types';

let conversations = $state<Conversation[]>([]);
let loading = $state(false);
let activeId = $state<string | null>(null);

export const conversationsStore = {
  get conversations() { return conversations; },
  get loading() { return loading; },
  get activeId() { return activeId; },
  setActive(id: string | null) { activeId = id; },
  async load() {
    loading = true;
    try {
      conversations = await listConversations();
    } catch {
      conversations = [];
    } finally {
      loading = false;
    }
  },
  async create(agentName?: string): Promise<Conversation> {
    const conv = await createConversation(agentName);
    conversations = [conv, ...conversations];
    return conv;
  },
  async remove(id: string) {
    await deleteConversation(id);
    conversations = conversations.filter((c) => c.id !== id);
    if (activeId === id) activeId = null;
  },
};
