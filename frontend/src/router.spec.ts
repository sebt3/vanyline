import { createMemoryHistory, createRouter, type Router } from 'vue-router';
import { describe, expect, it } from 'vitest';
import { routes } from './router';

function createTestRouter(): Router {
  return createRouter({
    history: createMemoryHistory(),
    routes,
  });
}

describe('router', () => {
  it('redirige / vers /settings', async () => {
    const router = createTestRouter();
    await router.push('/');
    await router.isReady();
    expect(router.currentRoute.value.path).toBe('/settings');
  });

  it('résout /ide/foo avec sandboxName', async () => {
    const router = createTestRouter();
    await router.push('/ide/foo');
    await router.isReady();
    expect(router.currentRoute.value.params.sandboxName).toBe('foo');
  });
});