import { mount } from '@vue/test-utils';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import IdeShell from './IdeShell.vue';

// Mock dockview-vue entièrement pour éviter dockview-core (ResizeObserver) en jsdom.
const mockPanelCloseSpy = vi.fn();
const mockGetPanelFn = vi.fn((id: string) => {
  if (id.startsWith('editor:')) {
    return { api: { close: vi.fn() } };
  }
  return null;
});

vi.mock('dockview-vue', () => ({
  DockviewVue: {
    template: '<div class="dockview-stub" />',
    emits: ['ready'],
    setup(_props: Record<string, unknown>, { emit }: { emit: (name: string, payload: unknown) => void }) {
      // Émettre 'ready' avec une fausse api pour que onReady s'exécute
      const fakeApi = {
        panels: [] as unknown[],
        getPanel: mockGetPanelFn,
        addPanel: vi.fn(),
        onDidLayoutChange: vi.fn(),
        activePanel: undefined,
        toJSON: () => null,
      };
      // Simuler ready après montage
      setTimeout(() => emit('ready', { api: fakeApi }), 0);
    },
  },
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
    mockGetPanelFn.mockClear();
    mockPanelCloseSpy.mockClear();
  });

  it('reçoit la prop sandboxName', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.props('sandboxName')).toBe('foo');
  });

  it('rend DockviewVue (mocké)', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.find('.dockview-stub').exists()).toBe(true);
  });

  it('crée un client /ws/fs partagé pour la sandbox', () => {
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

  it('close-file ferme l\'onglet du fichier renommé', async () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    // Attendre que l'événement ready soit émis (timeout 0)
    await new Promise((r) => setTimeout(r, 0));

    const { closeFile } = wrapper.vm as { closeFile: (p: string) => void };
    closeFile('src/main.py');

    // getPanel(a) devrait avoir été appelé avec 'editor:src/main.py'
    expect(mockGetPanelFn).toHaveBeenCalledWith('editor:src/main.py');
    const lastResult = mockGetPanelFn.mock.results[mockGetPanelFn.mock.results.length - 1];
    const panel = lastResult?.value as { api: { close: () => void } } | null;
    expect(panel).not.toBeNull();
    expect(panel!.api.close).toHaveBeenCalled();
  });

  it('close-file sans onglet ouvert ne plante pas', async () => {
    mockGetPanelFn.mockReturnValue(null);

    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    await new Promise((r) => setTimeout(r, 0));

    const { closeFile } = wrapper.vm as { closeFile: (p: string) => void };
    closeFile('inconnu.txt');

    // Aucune erreur, close jamais appelé
    expect(mockGetPanelFn).toHaveBeenCalledWith('editor:inconnu.txt');
  });

  describe('menu contextuel des onglets', () => {
    afterEach(() => {
      // Restaurer le navigator.clipboard originel en jsdom
      if (Object.prototype.hasOwnProperty.call(navigator, 'clipboard')) {
        delete (navigator as unknown as Record<string, unknown>).clipboard;
      }
    });

    it('menu des onglets : close/closeOthers/closeAll/séparateur + Copier le chemin pour un éditeur', async () => {
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const { getTabContextMenuItems } = wrapper.vm as {
        getTabContextMenuItems: (p: unknown) => unknown[];
      };
      const items = getTabContextMenuItems({ panel: { id: 'editor:src/main.py' } });

      const strs = items.filter((i): i is string => typeof i === 'string');
      expect(strs).toContain('close');
      expect(strs).toContain('closeOthers');
      expect(strs).toContain('closeAll');
      expect(strs).toContain('separator');
      const custom = items.find((i) => typeof i === 'object' && i !== null && 'label' in i!);
      expect(custom).not.toBeUndefined();
      expect((custom as { label: string })!.label).toBe('Copier le chemin');
    });

    it('Copier le chemin écrit le chemin du fichier', async () => {
      const writeText = vi.fn(() => Promise.resolve());
      Object.defineProperty(navigator, 'clipboard', {
        value: { writeText },
        configurable: true,
      });

      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const { getTabContextMenuItems } = wrapper.vm as {
        getTabContextMenuItems: (p: unknown) => unknown[];
      };
      const items = getTabContextMenuItems({ panel: { id: 'editor:src/main.py' } });
      const custom = items.find(
        (i) => typeof i === 'object' && i !== null && 'label' in i! && (i as { label: string }).label === 'Copier le chemin',
      );
      (custom as { action: () => void }).action();

      expect(writeText).toHaveBeenCalledWith('src/main.py');
    });

    it('pas de Copier le chemin pour un onglet non éditeur', async () => {
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const { getTabContextMenuItems } = wrapper.vm as {
        getTabContextMenuItems: (p: unknown) => unknown[];
      };
      const items = getTabContextMenuItems({ panel: { id: 'terminal' } });

      const custom = items.find(
        (i) => typeof i === 'object' && i !== null && 'label' in i! && (i as { label: string }).label === 'Copier le chemin',
      );
      expect(custom).toBeUndefined();
    });
  });
});
