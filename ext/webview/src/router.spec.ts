import { describe, expect, it } from 'vitest';
import { resolveView } from './router';

// resolveView est une fonction pure (pas de DOM) — pas de jsdom requis.
describe('resolveView (routeur webview)', () => {
  it("'config' explicite → config", () => {
    expect(resolveView('config')).toBe('config');
  });

  it("'chat' explicite → chat", () => {
    expect(resolveView('chat')).toBe('chat');
  });

  it('faute sécuritaire vers chat (valeur absente ou inconnue)', () => {
    expect(resolveView(null)).toBe('chat');
    expect(resolveView(undefined)).toBe('chat');
    expect(resolveView('nimportequoi')).toBe('chat');
  });
});
