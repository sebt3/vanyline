import { createMemoryHistory, createRouter } from 'vue-router';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { HealthReport, HealthState } from '@vanyline/protocol';
import StatusBar from './StatusBar.vue';
import AppBreadcrumb from './AppBreadcrumb.vue';

interface Call {
  method: string;
  url: string;
}

let calls: Call[] = [];
let healthReply: HealthState | null = null;

function mockHealthApi() {
  calls = [];
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? 'GET').toUpperCase();
    calls.push({ method, url });
    const ok = method === 'GET' && url === '/api/sandboxes/s1/code-health' && healthReply !== null;
    return {
      ok,
      status: ok ? 200 : 404,
      headers: new Map([['content-type', 'application/json']]),
      json: () => Promise.resolve(ok ? healthReply : { error: 'non mocké' }),
    } as unknown as Response;
  });
}

function healthState(overrides: Partial<HealthState>): HealthState {
  return {
    phase: 'fresh',
    report: {} as HealthReport,
    summary: { status: 'passed', coverage_percent: 82, files: [] },
    computed_at: 1,
    ...overrides,
  } as HealthState;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('StatusBar', () => {
  function mountStatusBar(props: { workspace?: string; extended?: boolean }): VueWrapper {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', name: 'home', component: { template: '<div>Home</div>' } }],
    });
    return mount(StatusBar, { props, global: { plugins: [router] } });
  }

  it('affiche toujours le breadcrumb', () => {
    const wrapper = mountStatusBar({});
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it("n'affiche pas les infos étendues quand extended est absent", () => {
    const wrapper = mountStatusBar({ workspace: 'media-station' });
    expect(wrapper.text()).not.toContain('media-station');
    expect(wrapper.text()).not.toContain('⎇ main');
    expect(wrapper.text()).not.toContain('UTF-8');
  });

  it('affiche le workspace et les infos étendues quand extended est vrai', () => {
    const wrapper = mountStatusBar({ workspace: 'media-station', extended: true });
    expect(wrapper.text()).toContain('media-station');
    expect(wrapper.text()).toContain('⎇ main');
    expect(wrapper.text()).toContain('UTF-8');
  });

  describe('indicateur de santé (route IDE)', () => {
    async function mountOnIde(): Promise<{ wrapper: VueWrapper; router: ReturnType<typeof createRouter> }> {
      const router = createRouter({
        history: createMemoryHistory(),
        routes: [
          { path: '/', name: 'home', component: { template: '<div>Home</div>' } },
          { path: '/p/:projectName/s/:sandboxName', name: 'ide', component: { template: '<div>IDE</div>' } },
          {
            path: '/p/:projectName/s/:sandboxName/overview',
            name: 'sandbox-overview',
            component: { template: '<div>Overview</div>' },
          },
        ],
      });
      await router.push('/p/demo/s/s1');
      await router.isReady();
      const wrapper = mount(StatusBar, { props: { workspace: 's1', extended: true }, global: { plugins: [router] } });
      await flushPromises();
      return { wrapper, router };
    }

    beforeEach(() => {
      healthReply = null;
      mockHealthApi();
    });

    it('indicateur_de_sante : « échec », « 61 % » et l\'indicateur « calcul en cours »', async () => {
      healthReply = healthState({
        phase: 'computing',
        summary: { status: 'failed', coverage_percent: 61, files: [] },
      });
      const { wrapper } = await mountOnIde();

      expect(wrapper.text()).toContain('échec');
      expect(wrapper.text()).toContain('61 %');
      expect(wrapper.text()).toContain('calcul en cours');
      expect(calls.some((c) => c.url === '/api/sandboxes/s1/code-health')).toBe(true);
      expect(calls.every((c) => c.url.startsWith('/api/'))).toBe(true);
    });

    it('clic_sur_l_indicateur : le détail de santé s\'ouvre', async () => {
      healthReply = healthState({
        phase: 'computing',
        summary: { status: 'failed', coverage_percent: 61, files: [] },
      });
      const { wrapper, router } = await mountOnIde();

      const indicator = wrapper.find('[data-testid="health-indicator"]');
      expect(indicator.exists()).toBe(true);
      await indicator.trigger('click');
      await flushPromises();

      expect(router.currentRoute.value.name).toBe('sandbox-overview');
      expect(router.currentRoute.value.path).toBe('/p/demo/s/s1/overview');
    });
  });
});
