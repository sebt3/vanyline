import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  clearIdeActions,
  registerIdeActions,
  useIdeSession,
} from './useIdeSession';

describe('useIdeSession', () => {
  beforeEach(() => {
    clearIdeActions();
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
});