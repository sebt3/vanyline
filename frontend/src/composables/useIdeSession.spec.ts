import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clearIdeActions,
  endAgentSession,
  registerIdeActions,
  startAgentSession,
  useIdeSession,
} from './useIdeSession';

const { fetchSpy } = vi.hoisted(() => ({ fetchSpy: vi.fn() }));
vi.stubGlobal('fetch', fetchSpy);

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

describe('useIdeSession', () => {
  beforeEach(() => {
    clearIdeActions();
    fetchSpy.mockReset();
  });

  afterEach(() => {
    clearIdeActions();
  });

  it('registerIdeActions fusionne au lieu de remplacer', () => {
    registerIdeActions({ saveActiveFile: () => {} });
    registerIdeActions({ closeActiveTab: () => {} });

    const { ideActions } = useIdeSession();
    expect(ideActions.value.saveActiveFile).toBeDefined();
    expect(ideActions.value.closeActiveTab).toBeDefined();
  });

  it('clearIdeActions réinitialise handlers, conversation et erreur', () => {
    registerIdeActions({ saveActiveFile: () => {} });
    const { activeConversationId, sessionError, ideActions } = useIdeSession();
    activeConversationId.value = 'conv-1';
    sessionError.value = 'boom';

    clearIdeActions();

    expect(ideActions.value).toEqual({});
    expect(activeConversationId.value).toBeNull();
    expect(sessionError.value).toBeNull();
  });

  it('startAgentSession : aucun agent configuré → sessionError, pas de conversation créée', async () => {
    fetchSpy.mockResolvedValueOnce(jsonResponse([]));

    await startAgentSession();

    const { activeConversationId, sessionError } = useIdeSession();
    expect(activeConversationId.value).toBeNull();
    expect(sessionError.value).toContain('Aucun agent configuré');
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it('startAgentSession : crée la conversation avec le premier agent', async () => {
    fetchSpy
      .mockResolvedValueOnce(jsonResponse([{ name: 'default' }, { name: 'other' }]))
      .mockResolvedValueOnce(jsonResponse({ id: 'conv-42' }));

    await startAgentSession();

    const { activeConversationId, sessionError } = useIdeSession();
    expect(activeConversationId.value).toBe('conv-42');
    expect(sessionError.value).toBeNull();

    const createCall = fetchSpy.mock.calls[1];
    expect(createCall[0]).toBe('/api/conversations');
    expect(JSON.parse(createCall[1].body)).toEqual({ agent_name: 'default' });
  });

  it('startAgentSession : erreur réseau → sessionError, conversation inchangée', async () => {
    fetchSpy.mockRejectedValueOnce(new Error('offline'));

    await startAgentSession();

    const { activeConversationId, sessionError } = useIdeSession();
    expect(activeConversationId.value).toBeNull();
    expect(sessionError.value).toBeTruthy();
  });

  it('endAgentSession referme la session sans toucher aux handlers', () => {
    registerIdeActions({ saveActiveFile: () => {} });
    const { activeConversationId, ideActions } = useIdeSession();
    activeConversationId.value = 'conv-1';

    endAgentSession();

    expect(activeConversationId.value).toBeNull();
    expect(ideActions.value.saveActiveFile).toBeDefined();
  });
});
