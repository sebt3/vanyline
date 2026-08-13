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
  it('/ résout sur / (plus de redirect)', async () => {
    const router = createTestRouter();
    await router.push('/');
    await router.isReady();
    expect(router.currentRoute.value.path).toBe('/');
  });

  it('/p/:projectName résout params.projectName et route.name', async () => {
    const router = createTestRouter();
    await router.push('/p/foo');
    await router.isReady();
    expect(router.currentRoute.value.params.projectName).toBe('foo');
    expect(router.currentRoute.value.name).toBe('project');
  });

  it('/p/:projectName/s/:sandboxName résout les deux params et route.name', async () => {
    const router = createTestRouter();
    await router.push('/p/foo/s/bar');
    await router.isReady();
    expect(router.currentRoute.value.params.sandboxName).toBe('bar');
    expect(router.currentRoute.value.params.projectName).toBe('foo');
    expect(router.currentRoute.value.name).toBe('ide');
  });

  it('/ide/foo ne matche aucune route', async () => {
    const router = createTestRouter();
    await router.push('/ide/foo');
    await router.isReady();
    expect(router.currentRoute.value.matched.length).toBe(0);
  });
});