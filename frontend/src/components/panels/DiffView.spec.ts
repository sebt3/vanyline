import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import { EditorView } from 'codemirror';
import DiffView from './DiffView.vue';

/** @codemirror/merge mock : `unifiedMergeView` retourne un tableau vide. */
const { unifiedMergeViewMock } = vi.hoisted(() => ({
  unifiedMergeViewMock: vi.fn(() => []),
}));

vi.mock('@codemirror/merge', () => ({
  unifiedMergeView: unifiedMergeViewMock,
}));

/** gitClient mock — dans hoisted car vi.mock est aussi hoisé. */
const { gitClientMock } = vi.hoisted(() => {
  const mock = {
    diff: vi.fn(),
  };
  return { gitClientMock: mock };
});

vi.mock('../../api/gitClient', () => ({
  gitClient: gitClientMock,
}));

function makeClient() {
  return {
    request: vi.fn().mockImplementation(
      async (op: string, _params: Record<string, unknown>) => {
        if (op === 'read') return { ok: true, content: 'ligne1\nligne2\n', truncated: false };
        return { ok: false, error: 'unknown' };
      },
    ),
  };
}

function makePanelApi(isActive = true) {
  return {
    isActive,
    onDidActiveChange: vi.fn(() => ({ dispose: vi.fn() })),
  } as unknown as import('dockview-vue').DockviewPanelApi;
}

function diffViewProps(path: string, staged = false, isActive = true) {
  return { params: { params: { path, staged }, api: makePanelApi(isActive) } };
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('DiffView.vue', () => {
  let client: ReturnType<typeof makeClient>;

  beforeEach(() => {
    client = makeClient();
    gitClientMock.diff.mockReset();
    unifiedMergeViewMock.mockReset();
    document.body.innerHTML = '';
  });

  afterEach(() => {
    // Nettoyage global
    document.body.innerHTML = '';
  });

  it('charge le diff et rend le merge unifié (HEAD → working tree)', async () => {
    const patch = [
      'diff --git a/a.txt b/a.txt',
      'index ..1..2.. 100644',
      '@@ -1,3 +1,3 @@',
      ' ligne1',
      '-ligne2',
      '+ligne2 MODIFIEE',
      ' ligne3',
    ].join('\n');

    gitClientMock.diff.mockResolvedValueOnce({ path: 'a.txt', diff: patch });
    client.request.mockImplementation(
      async (op, _params) => {
        if (op === 'read') return { ok: true, content: 'ligne1\nligne2 MODIFIEE\nligne3\n', truncated: false };
        return { ok: false, error: 'unknown' };
      },
    );

    const wrapper = mount(DiffView, {
      props: diffViewProps('a.txt'),
      global: { provide: { 'sandbox-fs': ref(client), 'sandbox-name': 's' } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(gitClientMock.diff).toHaveBeenCalledWith('s', 'a.txt', false);

    // unifiedMergeView a été appelé avec le base reconstruit
    expect(unifiedMergeViewMock).toHaveBeenCalledWith({ original: 'ligne1\nligne2\nligne3\n' });

    // Le document du view est le contenu working tree
    const { getView } = wrapper.vm as { getView: () => EditorView };
    expect(getView().state.doc.toString()).toBe('ligne1\nligne2 MODIFIEE\nligne3\n');
  });

  it('staged=true est transmis à gitClient.diff', async () => {
    const patch = '@@ -1,1 +1,1 @@\n-old\n+new\n';
    gitClientMock.diff.mockResolvedValueOnce({ path: 'a.txt', diff: patch });
    client.request.mockImplementation(
      async (op, _params) => {
        if (op === 'read') return { ok: true, content: 'new\n', truncated: false };
        return { ok: false, error: 'unknown' };
      },
    );

    mount(DiffView, {
      props: diffViewProps('a.txt', true),
      global: { provide: { 'sandbox-fs': ref(client), 'sandbox-name': 's' } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(gitClientMock.diff).toHaveBeenCalledWith('s', 'a.txt', true);
  });

  it('patch vide → rendu simple sans unifiedMergeView', async () => {
    gitClientMock.diff.mockResolvedValueOnce({ path: 'a.txt', diff: '' });
    client.request.mockImplementation(
      async (op, _params) => {
        if (op === 'read') return { ok: true, content: 'seul conten\n', truncated: false };
        return { ok: false, error: 'unknown' };
      },
    );

    const wrapper = mount(DiffView, {
      props: diffViewProps('a.txt'),
      global: { provide: { 'sandbox-fs': ref(client), 'sandbox-name': 's' } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    // unifiedMergeView n'a PAS été appelé
    expect(unifiedMergeViewMock).not.toHaveBeenCalled();

    // Le document contient le contenu working tree
    const { getView } = wrapper.vm as { getView: () => EditorView };
    expect(getView().state.doc.toString()).toBe('seul conten\n');
  });

  it('échec read → message d\'état, pas de rendu', async () => {
    gitClientMock.diff.mockResolvedValueOnce({ path: 'a.txt', diff: 'hunk' });
    client.request.mockRejectedValueOnce(new Error('Not found'));

    const wrapper = mount(DiffView, {
      props: diffViewProps('a.txt'),
      global: { provide: { 'sandbox-fs': ref(client), 'sandbox-name': 's' } },
    });

    await flushMicrotasks();
    await flushMicrotasks();

    // Il y a un élément de status avec role="alert"
    const alert = wrapper.find('[role="alert"]');
    expect(alert.exists()).toBe(true);

    // unifiedMergeView n'a pas été appelé
    expect(unifiedMergeViewMock).not.toHaveBeenCalled();
  });
});