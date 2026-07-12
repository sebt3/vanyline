import { describe, it, expect, vi } from 'vitest';
import * as conversationsApi from '$lib/api/conversations';
import { conversationsStore } from './conversations.svelte';

describe('conversationsStore', () => {
  const mockConv = {
    id: 'conv-1',
    user_id: 'user-1',
    agent_name: 'my-agent',
    title: null,
    created_at: '2025-01-01T00:00:00Z',
    updated_at: '2025-01-01T00:00:00Z',
  } as any;

  it('create_sends_agent_name', async () => {
    const spy = vi.spyOn(conversationsApi, 'createConversation').mockResolvedValue(mockConv);

    const result = await conversationsStore.create('my-agent');

    expect(spy).toHaveBeenCalledWith('my-agent');
    expect(result).toBe(mockConv);
    expect(conversationsStore.conversations).toContainEqual(mockConv);
  });

  it('create_without_agent_passes_undefined', async () => {
    const spy = vi.spyOn(conversationsApi, 'createConversation').mockResolvedValue(mockConv);

    await conversationsStore.create();

    expect(spy).toHaveBeenCalledWith(undefined);
  });
});