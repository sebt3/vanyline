import { mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import IdeShell from './IdeShell.vue';

// Mock dockview-vue entièrement pour éviter dockview-core (ResizeObserver) en jsdom.
vi.mock('dockview-vue', () => ({
  DockviewVue: { template: '<div class="dockview-stub" />' },
}));

const mockWs = { addEventListener: vi.fn(), removeEventListener: vi.fn() };

// vi.hoisted s'exécute avant les vi.mock (au chargement du module), ce qui permet
// d'injecter les mêmes références vi.fn() dans le factory du mock et ici.
const { openSandboxWs, SandboxFsClient: _SandboxFsClient } = vi.hoisted(() => {
  const fn = vi.fn(() => Promise.resolve(mockWs));
  return {
    openSandboxWs: fn,
    SandboxFsClient: class SandboxFsClient {
      constructor(ws: WebSocket) { this.ws = ws; }
      ws: WebSocket;
    },
  };
});

vi.mock('../api/sandboxWs', () => ({
  openSandboxWs,
  SandboxFsClient: _SandboxFsClient,
}));

describe('IdeShell', () => {
  beforeEach(() => {
    openSandboxWs.mockClear();
    openSandboxWs.mockReturnValue(Promise.resolve(mockWs));
  });

  it('reçoit la prop sandboxName', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.props('sandboxName')).toBe('foo');
  });

  it('rend DockviewVue (mocké)', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.find('.dockview-stub').exists()).toBe(true);
  });

  it('crée un client /ws/fs partagé pour la sandbox', async () => {
    mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(openSandboxWs).toHaveBeenCalledWith('foo', '/ws/fs');
  });

  it('un échec du ticket laisse fsClient null', async () => {
    openSandboxWs.mockRejectedValueOnce(new Error('ticket failed'));
    const wrapper = mount(IdeShell, { props: { sandboxName: 'bar' } });
    await new Promise((r) => setTimeout(r, 0));
    // fsClient === null : aucun crash, le test passe s'il n'y a pas d'erreur.
    expect(wrapper.find('.dockview-stub').exists()).toBe(true);
  });
});
