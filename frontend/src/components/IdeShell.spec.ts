import { mount } from '@vue/test-utils';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import IdeShell from './IdeShell.vue';

// Mock ../api/lsp pour que IdeShell provide() et disposeLspClients
// ne dépendent pas d'une implémentation réelle. Utilisation de vi.hoisted
// car vi.mock est ascenseur (hoisted) : les variables doivent exister
// avant que le mock ne soit évalué.
const { getLspClient, disposeLspClients } = vi.hoisted(() => ({
  getLspClient: vi.fn(async () => null),
  disposeLspClients: vi.fn(),
}));

vi.mock('../api/lsp', () => ({
  getLspClient,
  disposeLspClients,
}));

// Mock dockview-vue entièrement pour éviter dockview-core (ResizeObserver) en jsdom.
const mockPanelCloseSpy = vi.fn();
const mockGetPanelFn = vi.fn((id: string) => {
  if (id.startsWith('editor:')) {
    return { api: { close: vi.fn() } };
  }
  return null;
});
const mockAddPanel = vi.fn();

vi.mock('dockview-vue', () => ({
  DockviewVue: {
    template: '<div class="dockview-stub" />',
    emits: ['ready'],
    setup(_props: Record<string, unknown>, { emit }: { emit: (name: string, payload: unknown) => void }) {
      // Émettre 'ready' avec une fausse api pour que onReady s'exécute
      const fakeApi = {
        panels: [] as unknown[],
        getPanel: mockGetPanelFn,
        addPanel: mockAddPanel,
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
// fsOnEvent : espion partagé de `SandboxFsClient.onEvent` (push file-changed,
// tâche 08b) — toutes les instances du mock appellent le même vi.fn().
const { openSandboxWs, SandboxFsClient: _SandboxFsClient, fsOnEvent } = vi.hoisted(() => {
  const fn = vi.fn(() => Promise.resolve(mockWs));
  const onEventSpy = vi.fn();
  return {
    openSandboxWs: fn,
    fsOnEvent: onEventSpy,
    SandboxFsClient: class SandboxFsClient {
      constructor(ws: WebSocket) { this.ws = ws; }
      ws: WebSocket;
      onEvent = onEventSpy;
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
    fsOnEvent.mockClear();
    mockGetPanelFn.mockClear();
    mockAddPanel.mockClear();
    mockPanelCloseSpy.mockClear();
    getLspClient.mockClear();
    disposeLspClients.mockClear();
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

  it('abonne le handler file-changed sur le client /ws/fs créé (08b)', async () => {
    mount(IdeShell, { props: { sandboxName: 'foo' } });
    // Laisse la chaîne openSandboxWs().then(...) construire le client.
    await new Promise((r) => setTimeout(r, 0));

    expect(fsOnEvent).toHaveBeenCalledWith('file-changed', expect.any(Function));
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

  it('crée le panel Git par défaut', async () => {
    mount(IdeShell, { props: { sandboxName: 'foo' } });
    await new Promise((r) => setTimeout(r, 0));

    const gitCall = mockAddPanel.mock.calls.find(
      (c): c is [{ id: string; component: string }] =>
        c[0] != null && c[0].id === 'git' && c[0].component === 'git',
    );
    expect(gitCall).not.toBeUndefined();
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

  it('openDiff crée un panel diff:<path>', async () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    await new Promise((r) => setTimeout(r, 0));

    const { openDiff } = wrapper.vm as {
      openDiff: (p: string, s?: boolean) => void;
    };
    openDiff('a.txt', true);

    const addCall = mockAddPanel.mock.calls.find(
      (c): c is [{ id: string; component: string; params: unknown }] =>
        c[0] != null &&
        (c[0] as { id: string }).id === 'diff:a.txt' &&
        (c[0] as { component: string }).component === 'diff',
    );
    expect(addCall).not.toBeUndefined();
    expect(addCall![0].params).toEqual({ path: 'a.txt', staged: true });
  });

  it('démonte et dispose les clients LSP de la sandbox', async () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    await new Promise((r) => setTimeout(r, 0));

    wrapper.unmount();

    expect(disposeLspClients).toHaveBeenCalledWith('foo');
  });
});
