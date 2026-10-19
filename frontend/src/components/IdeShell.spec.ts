import { mount } from '@vue/test-utils';
import { inject, nextTick, type Component } from 'vue';
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

// Valeur injectée `project-name` telle que vue la voit depuis le stub dockview (enfant d'IdeShell).
const projectNameSeen = vi.hoisted(() => ({ value: undefined as unknown }));

// Ce que IdeShell passe à DockviewVue (attributs non déclarés par le stub) et le
// registre `tabComponents` : permet de monter le composant d'onglet réel.
const dockviewState = vi.hoisted(() => ({
  attrs: {} as Record<string, unknown>,
  tabComponents: {} as Record<string, unknown>,
}));

vi.mock('dockview-vue', () => ({
  DockviewVue: {
    template: '<div class="dockview-stub" />',
    inheritAttrs: false,
    emits: ['ready'],
    setup(_props: Record<string, unknown>, { emit, attrs }: { emit: (name: string, payload: unknown) => void; attrs: Record<string, unknown> }) {
      projectNameSeen.value = inject('project-name', null);
      dockviewState.attrs = { ...attrs };
      dockviewState.tabComponents = (attrs['tab-components'] ?? attrs.tabComponents ?? {}) as Record<string, unknown>;
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

type FakePanel = { id: string; api: { close: ReturnType<typeof vi.fn> } };

function makePanel(id: string): FakePanel {
  return { id, api: { close: vi.fn() } };
}

/** Faux onglet dockview : api du panneau + groupe (panels) + close du panneau. */
function makeTab(id: string, others: FakePanel[] = []) {
  const close = vi.fn();
  const self: FakePanel = { id, api: { close } };
  const panels = [self, ...others];
  const title = id.startsWith('editor:') ? (id.slice('editor:'.length).split('/').pop() ?? id) : id;
  const api = {
    id,
    title,
    close,
    group: { panels },
    onDidTitleChange: vi.fn(() => ({ dispose: vi.fn() })),
  };
  return { api, self, close };
}

function mountTab(api: unknown) {
  const Tab = dockviewState.tabComponents.tab as Component;
  return mount(Tab, {
    props: { api, containerApi: {}, params: {}, tabLocation: 'header' } as Record<string, unknown>,
    attachTo: document.body,
  });
}

async function flushUi() {
  await nextTick();
  await new Promise((r) => setTimeout(r, 0));
}

/** Clic droit sur le titre de l'onglet, puis lecture des entrées du menu ouvert. */
async function openMenu(wrapper: ReturnType<typeof mountTab>): Promise<HTMLElement[]> {
  wrapper.find('.dv-default-tab-content').element.dispatchEvent(
    new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
  );
  await flushUi();
  return [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')];
}

function clickItem(items: HTMLElement[], label: string) {
  const item = items.find((el) => el.textContent?.trim() === label);
  expect(item).toBeTruthy();
  item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

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

  it('project-name fourni est réactif (ref) : le panneau Workflows suit le changement de projet', async () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo', projectName: 'demo' } });
    const injected = projectNameSeen.value as { value: string };
    expect(injected).toBeTruthy();
    expect(injected.value).toBe('demo');
    await wrapper.setProps({ projectName: 'autre' });
    expect(injected.value).toBe('autre');
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

  it('abonne le handler flush-request sur le client /ws/fs créé (08c)', async () => {
    mount(IdeShell, { props: { sandboxName: 'foo' } });
    await new Promise((r) => setTimeout(r, 0));

    // Aller-retour « flush avant écriture » (cas B, R1 sq3) : l'abonnement
    // vit à côté de celui de file-changed, même client, même registre.
    expect(fsOnEvent).toHaveBeenCalledWith('flush-request', expect.any(Function));
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

  describe('menu contextuel des onglets (ide.sdd)', () => {
    afterEach(() => {
      // Restaurer le navigator.clipboard originel en jsdom
      if (Object.prototype.hasOwnProperty.call(navigator, 'clipboard')) {
        delete (navigator as unknown as Record<string, unknown>).clipboard;
      }
      document.body.innerHTML = '';
    });

    it('menu_des_onglets_sans_module_enterprise', async () => {
      const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
      const error = vi.spyOn(console, 'error').mockImplementation(() => {});
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      // Aucune prop getTabContextMenuItems passée à dockview (ni en kebab-case ni en camelCase).
      const keys = Object.keys(dockviewState.attrs);
      expect(keys).not.toContain('get-tab-context-menu-items');
      expect(keys).not.toContain('getTabContextMenuItems');
      // Le composant d'onglet maison est enregistré pour les onglets par défaut.
      expect(dockviewState.tabComponents.tab).toBeTruthy();
      // Aucun avertissement « requires the ContextMenu module » (dockview-enterprise).
      const messages = [...warn.mock.calls, ...error.mock.calls].map((args) => args.map(String).join(' '));
      expect(messages.some((m) => m.includes('ContextMenu'))).toBe(false);
      wrapper.unmount();
    });

    it('clic_droit_sur_un_onglet_editeur', async () => {
      const writeText = vi.fn(() => Promise.resolve());
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const other = makePanel('editor:src/lib.rs');
      const tab = makeTab('editor:src/main.rs', [other]);
      const tabWrapper = mountTab(tab.api);
      const items = await openMenu(tabWrapper);

      const labels = items.map((el) => el.textContent?.trim());
      expect(labels).toEqual(['Fermer', 'Fermer les autres', 'Fermer tout', 'Copier le chemin']);
      expect(document.querySelector('[role="separator"]')).toBeTruthy();

      clickItem(items, 'Copier le chemin');
      await flushUi();
      expect(writeText).toHaveBeenCalledWith('src/main.rs');

      tabWrapper.unmount();
      wrapper.unmount();
    });

    it('Fermer les autres ferme les autres onglets du groupe, pas celui du clic', async () => {
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const other = makePanel('terminal-2');
      const tab = makeTab('terminal', [other]);
      const tabWrapper = mountTab(tab.api);
      clickItem(await openMenu(tabWrapper), 'Fermer les autres');
      await flushUi();

      expect(other.api.close).toHaveBeenCalledTimes(1);
      expect(tab.close).not.toHaveBeenCalled();
      tabWrapper.unmount();
      wrapper.unmount();
    });

    it('onglet personnalisé : titre affiché et bouton de fermeture fonctionnel', async () => {
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const tab = makeTab('editor:src/main.rs');
      const tabWrapper = mountTab(tab.api);
      expect(tabWrapper.find('.dv-default-tab-content').text()).toBe('main.rs');

      await tabWrapper.find('.dv-default-tab-action').trigger('click');
      expect(tab.close).toHaveBeenCalledTimes(1);
      tabWrapper.unmount();
      wrapper.unmount();
    });

    it('pas de Copier le chemin pour un onglet non éditeur', async () => {
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const tab = makeTab('terminal');
      const tabWrapper = mountTab(tab.api);
      const labels = (await openMenu(tabWrapper)).map((el) => el.textContent?.trim());
      expect(labels).toEqual(['Fermer', 'Fermer les autres', 'Fermer tout']);
      tabWrapper.unmount();
      wrapper.unmount();
    });

    it('Copier le chemin écrit le chemin du fichier', async () => {
      const writeText = vi.fn(() => Promise.resolve());
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
      const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
      await new Promise((r) => setTimeout(r, 0));

      const tab = makeTab('editor:src/main.py');
      const tabWrapper = mountTab(tab.api);
      clickItem(await openMenu(tabWrapper), 'Copier le chemin');
      await flushUi();

      expect(writeText).toHaveBeenCalledWith('src/main.py');
      tabWrapper.unmount();
      wrapper.unmount();
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
