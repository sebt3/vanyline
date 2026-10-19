import { createMemoryHistory, createRouter, type Router } from 'vue-router';
import { describe, expect, it } from 'vitest';
import { installRouteGuards, routes } from './router';

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

  it('/p/:projectName/s/:sandboxName/overview résout la vue de sandbox', async () => {
    const router = createTestRouter();
    await router.push('/p/foo/s/bar/overview');
    await router.isReady();
    expect(router.currentRoute.value.name).toBe('sandbox-overview');
    expect(router.currentRoute.value.params.sandboxName).toBe('bar');
  });

  it('/p/:projectName/runs/:runId résout le détail d un run', async () => {
    const router = createTestRouter();
    await router.push('/p/foo/runs/r42');
    await router.isReady();
    expect(router.currentRoute.value.name).toBe('run');
    expect(router.currentRoute.value.params.runId).toBe('r42');
  });

  it('ide_redirige_sous_lg', async () => {
    const router = createTestRouter();
    installRouteGuards(router, () => 800);
    await router.push('/p/demo/s/s1');
    await router.isReady();
    expect(router.currentRoute.value.path).toBe('/p/demo/s/s1/overview');
  });

  it('ide_disponible_a_partir_de_lg', async () => {
    const router = createTestRouter();
    installRouteGuards(router, () => 1280);
    await router.push('/p/demo/s/s1');
    await router.isReady();
    expect(router.currentRoute.value.path).toBe('/p/demo/s/s1');
    expect(router.currentRoute.value.name).toBe('ide');
  });

  it('/ide/foo ne matche aucune route', async () => {
    const router = createTestRouter();
    await router.push('/ide/foo');
    await router.isReady();
    expect(router.currentRoute.value.matched.length).toBe(0);
  });
});
