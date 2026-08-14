import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import Editor from './Editor.vue';
import { clearIdeActions, useIdeSession } from '../../composables/useIdeSession';
import type { DockviewPanelApi } from 'dockview-vue';

function makeClient() {
  return {
    request: vi.fn().mockImplementation(
      async (op: string, params: Record<string, unknown>) => {
        if (op === 'read') return { ok: true, content: 'ligne1\nligne2\n', truncated: false };
        if (op === 'write') return { ok: true, wrote: params.path as string };
        return { ok: false, error: 'unknown' };
      },
    ),
  };
}

/** Un panel Editor par fichier (task multi-onglets) : `path` est fixe pour la
 *  durée de vie de l'instance, reçu via `params`/`api` (props dockview),
 *  plus `sandbox-fs` toujours injecté par IdeShell. */
function makePanelApi(isActive = true): DockviewPanelApi {
  return {
    isActive,
    onDidActiveChange: vi.fn(() => ({ dispose: vi.fn() })),
  } as unknown as DockviewPanelApi;
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('Editor.vue — contenu réel', () => {
  let client: ReturnType<typeof makeClient>;

  beforeEach(() => {
    client = makeClient();
  });

  it('lit le fichier de params.path au montage (raw: true)', async () => {
    const wrapper = mount(Editor, {
      props: { params: { path: 'src/main.py' }, api: makePanelApi() },
      global: { provide: { 'sandbox-fs': ref(client) } },
    });

    // 2 flush : onMounted + read async résolu
    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('read', {
      path: 'src/main.py',
      raw: true,
    });

    expect(wrapper.element.textContent).toContain('ligne1');
  });

  it('Ctrl+S écrit le contenu courant', async () => {
    const wrapper = mount(Editor, {
      props: { params: { path: 'src/main.py' }, api: makePanelApi() },
      global: { provide: { 'sandbox-fs': ref(client) } },
    });

    // 2 flush : onMounted(async read) + read async résolu
    await flushMicrotasks();
    await flushMicrotasks();

    const { save } = wrapper.vm as { save: () => void };
    save();

    await flushMicrotasks();

    // read appelé en premier (montage), write depuis save()
    expect(client.request).toHaveBeenCalledWith('write', {
      path: 'src/main.py',
      content: 'ligne1\nligne2\n',
    });
  });

  it("n'enregistre saveActiveFile que pour l'onglet actif (plusieurs instances possibles)", async () => {
    const { ideActions } = useIdeSession();
    clearIdeActions();

    mount(Editor, {
      props: { params: { path: 'inactive.py' }, api: makePanelApi(false) },
      global: { provide: { 'sandbox-fs': ref(makeClient()) } },
    });
    await flushMicrotasks();
    expect(ideActions.value.saveActiveFile).toBeUndefined();

    const activeWrapper = mount(Editor, {
      props: { params: { path: 'active.py' }, api: makePanelApi(true) },
      global: { provide: { 'sandbox-fs': ref(client) } },
    });
    await flushMicrotasks();
    await flushMicrotasks();

    expect(ideActions.value.saveActiveFile).toBeDefined();
    ideActions.value.saveActiveFile?.();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('write', {
      path: 'active.py',
      content: 'ligne1\nligne2\n',
    });
    // Vérifie que c'est bien l'instance active qui a écrit, pas l'inactive.
    expect((activeWrapper.vm as { save: () => void }).save).toBeDefined();
  });
});
