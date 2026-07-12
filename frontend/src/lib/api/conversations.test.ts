import { describe, it, expect, vi } from 'vitest';
import { createConversation } from './conversations';

describe('createConversation', () => {
  it('createConversation_sends_agent_name_field', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: () => Promise.resolve({
        id: 'conv-1',
        user_id: 'user-1',
        agent_name: 'test-agent',
        title: null,
        created_at: '2025-01-01T00:00:00Z',
        updated_at: '2025-01-01T00:00:00Z',
      }),
    } as Response);

    vi.stubGlobal('fetch', mockFetch);

    await createConversation('test-agent');

    expect(mockFetch).toHaveBeenCalled();
    const [url, options] = mockFetch.mock.calls[0] as [string, RequestInit | undefined];
    expect(url).toBe('/api/conversations');
    const body = JSON.parse(options?.body as string);
    expect(body).toEqual({ agent_name: 'test-agent' });

    vi.unstubAllGlobals();
  });
});