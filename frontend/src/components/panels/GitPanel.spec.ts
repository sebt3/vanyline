import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import GitPanel from './GitPanel.vue';
import type { LogCommit, LogResult } from '../../api/gitClient';

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

// vi.hoisted ensures these are defined at the hoisted level (before vi.mock runs)
const { mockClient } = vi.hoisted(() => {
  function makeMockGitClient() {
    return {
      status: vi.fn(),
      branches: vi.fn(),
      stage: vi.fn(),
      unstage: vi.fn(),
      commit: vi.fn(),
      unpushed: vi.fn().mockResolvedValue({
        branch: 'main',
        upstream: 'origin/main',
        commits: [],
        truncated: false,
      }),
      push: vi.fn(),
      createBranch: vi.fn(),
      checkout: vi.fn(),
      deleteBranch: vi.fn(),
      log: vi.fn(async () => ({ branch: 'main', commits: [] as LogCommit[], truncated: false })),
    };
  }
  return { mockClient: makeMockGitClient() };
});

vi.mock('../../api/gitClient', () => ({
  gitClient: mockClient,
}));

// Mock @gitgraph/js : le composant appelle createGitgraph à chaque refresh.
// Retour par défaut minimal pour les tests existants (statut/staging/commit) ;
// les tests du graphe re-configurent mockReturnValue avec des vi.fn locaux.
const { createGitgraphMock } = vi.hoisted(() => ({ createGitgraphMock: vi.fn() }));

vi.mock('@gitgraph/js', () => ({
  createGitgraph: createGitgraphMock,
}));

createGitgraphMock.mockReturnValue({
  branch: vi.fn(() => ({
    commit: vi.fn(() => ({ tag: vi.fn() })),
  })),
});

describe('GitPanel.vue — statut, staging, commit', () => {
  beforeEach(() => {
    mockClient.status.mockReset();
    mockClient.branches.mockReset();
    mockClient.stage.mockReset();
    mockClient.unstage.mockReset();
    mockClient.commit.mockReset();
    mockClient.unpushed.mockReset();
    mockClient.push.mockReset();
    mockClient.createBranch.mockReset();
    mockClient.checkout.mockReset();
    mockClient.deleteBranch.mockReset();
    mockClient.log.mockReset();
  });

  it('affiche staged, unstaged et conflicted depuis le statut', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'a.txt', state: 'modified', staged: true },
        { path: 'b.txt', state: 'modified', staged: false },
        { path: 'c.txt', state: 'conflicted', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    // Tous les chemins doivent être affichés
    expect(wrapper.text()).toContain('a.txt');
    expect(wrapper.text()).toContain('b.txt');
    expect(wrapper.text()).toContain('c.txt');

    // Les états doivent correspondre
    expect(wrapper.text()).toContain('staged');
    expect(wrapper.text()).toContain('modified');
    expect(wrapper.text()).toContain('conflit');

    // Bouton « Marquer résolu » ABSENT car merging=false
    expect(wrapper.text()).not.toContain('Marquer résolu');

    // Stager présent pour b.txt
    expect(wrapper.text()).toContain('Stager');
  });

  it('cliquer Stager appelle gitClient.stage([path])', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'b.txt', state: 'modified', staged: false },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    // stage + refresh.status + refresh.branches
    mockClient.stage.mockResolvedValueOnce({ ok: true });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'b.txt', state: 'modified', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const stageBtn = wrapper.findAll('button').find((b) => b.text() === 'Stager');
    expect(stageBtn).toBeDefined();
    await stageBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.stage).toHaveBeenCalledWith('s', ['b.txt']);
  });

  it('cliquer Marquer résolu appelle gitClient.stage([path])', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'c.txt', state: 'conflicted', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: true,
      branches: [],
    });

    mockClient.stage.mockResolvedValueOnce({ ok: true });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'c.txt', state: 'conflicted', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: true,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    // « Marquer résolu » présent quand merging=true
    expect(wrapper.text()).toContain('Marquer résolu');

    const resolvedBtn = wrapper.findAll('button').find((b) => b.text() === 'Marquer résolu');
    expect(resolvedBtn).toBeDefined();
    await resolvedBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.stage).toHaveBeenCalledWith('s', ['c.txt']);
  });

  it('saisir un message puis Commit appelle gitClient.commit', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'a.txt', state: 'modified', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    // commit + refresh
    mockClient.commit.mockResolvedValueOnce({ sha: 'abc123', title: '' });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    // Remplir le textarea
    const textarea = wrapper.find('textarea');
    expect(textarea.exists()).toBe(true);
    await textarea.setValue('feat: ajouter la fonctionnalité');

    const commitBtn = wrapper.findAll('button').find((b) => b.text() === 'Commit');
    expect(commitBtn).toBeDefined();
    await commitBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.commit).toHaveBeenCalledWith('s', 'feat: ajouter la fonctionnalité');
  });

  it('bouton Commit désactivé si rien staged ou message vide', async () => {
    // Cas 1 : status clean + message vide
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const commitBtn = wrapper.findAll('button').find((b) => b.text() === 'Commit');
    expect(commitBtn?.attributes('disabled')).toBeDefined();

    // Cas 2 : message non vide mais rien staged
    await wrapper.find('textarea').setValue('msg');
    await flushMicrotasks();

    // canCommit = false car stagedFiles + conflictedFiles = 0
    expect(commitBtn?.attributes('disabled')).toBeDefined();
  });

  it('affiche le compteur de commits non poussés', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [
        { sha: 'aabbccd', title: 'feat: first', author: 'seb', date: '2026-01-01T00:00:00Z' },
        { sha: 'eeffggh', title: 'fix: second', author: 'seb', date: '2026-01-02T00:00:00Z' },
      ],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    expect(wrapper.text()).toContain('2 commits non poussés');

    const pushBtn = wrapper.findAll('button').find((b) => b.text() === 'Push');
    expect(pushBtn).toBeDefined();
    expect(pushBtn?.attributes('disabled')).toBeUndefined();
  });

  it('cliquer Push appelle gitClient.push puis refresh', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [{ sha: 'abc1234', title: 'msg', author: 'seb', date: '2026-01-01T00:00:00Z' }],
      truncated: false,
    });

    // push → refresh (status + branches + unpushed)
    mockClient.push.mockResolvedValueOnce({ ok: true, pushed: 1 });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const pushBtn = wrapper.findAll('button').find((b) => b.text() === 'Push');
    expect(pushBtn).toBeDefined();
    await pushBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.push).toHaveBeenCalledWith('s');
    expect(mockClient.status).toHaveBeenCalled();
  });

  it('créer une branche appelle gitClient.createBranch sans from', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    // createBranch → refresh
    mockClient.createBranch.mockResolvedValueOnce({ ok: true });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const inputs = wrapper.findAll('input');
    const [branchInput, fromInput] = [inputs[0], inputs[1]];
    expect(branchInput).toBeDefined();
    expect(fromInput).toBeDefined();
    await branchInput!.setValue('feat/x');
    await fromInput!.setValue('');

    const createBtn = wrapper.findAll('button').find((b) => b.text() === 'Créer');
    expect(createBtn).toBeDefined();
    await createBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.createBranch).toHaveBeenCalledWith('s', 'feat/x', undefined);
  });

  it('créer une branche avec from appelle createBranch avec from', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    // createBranch → refresh
    mockClient.createBranch.mockResolvedValueOnce({ ok: true });
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const inputs = wrapper.findAll('input');
    const [branchInput, fromInput] = [inputs[0], inputs[1]];
    await branchInput!.setValue('feat/x');
    await fromInput!.setValue('origin/main');

    const createBtn = wrapper.findAll('button').find((b) => b.text() === 'Créer');
    await createBtn?.trigger('click');

    await flushMicrotasks();

    expect(mockClient.createBranch).toHaveBeenCalledWith('s', 'feat/x', 'origin/main');
  });

  it('cliquer Switcher appelle gitClient.checkout', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [
        { name: 'main', is_remote: false, upstream: null },
        { name: 'feat/x', is_remote: false, upstream: null },
      ],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    // checkout → refresh
    mockClient.checkout.mockResolvedValueOnce({ ok: true });
    mockClient.status.mockResolvedValueOnce({
      branch: 'feat/x',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'feat/x',
      merging: false,
      branches: [
        { name: 'main', is_remote: false, upstream: null },
        { name: 'feat/x', is_remote: false, upstream: null },
      ],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'feat/x',
      upstream: null,
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const switchBtns = wrapper.findAll('button').filter((b) => b.text() === 'Switcher');
    expect(switchBtns.length).toBeGreaterThanOrEqual(2);
    // Le deuxième Switcher (index 1) est sur feat/x, le premier est sur main (courante, disabled)
    await switchBtns[1].trigger('click');

    await flushMicrotasks();

    expect(mockClient.checkout).toHaveBeenCalledWith('s', 'feat/x');
  });

  it('cliquer Supprimer appelle gitClient.deleteBranch', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [
        { name: 'main', is_remote: false, upstream: null },
        { name: 'feat/x', is_remote: false, upstream: null },
      ],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    // deleteBranch → refresh
    mockClient.deleteBranch.mockResolvedValueOnce(undefined);
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [
        { name: 'main', is_remote: false, upstream: null },
        { name: 'feat/x', is_remote: false, upstream: null },
      ],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const deleteBtns = wrapper.findAll('button').filter((b) => b.text() === 'Supprimer');
    expect(deleteBtns.length).toBeGreaterThanOrEqual(2);
    // Le deuxième Supprimer (index 1) est sur feat/x
    await deleteBtns[1].trigger('click');

    await flushMicrotasks();

    expect(mockClient.deleteBranch).toHaveBeenCalledWith('s', 'feat/x');
  });

  it('bouton Push désactivé quand aucun commit non poussé', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const pushBtn = wrapper.findAll('button').find((b) => b.text() === 'Push');
    expect(pushBtn?.attributes('disabled')).toBeDefined();
  });
});

describe('GitPanel.vue — graphe historique', () => {
  it('charge le log et expose les ref graphe pour un rendu ultérieur', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    mockClient.log.mockResolvedValueOnce({
      branch: 'main',
      commits: [
        {
          sha: 'c2',
          parents: ['c1'],
          refs: ['HEAD', 'v2'],
          title: 'Second commit',
          author: 'seb <seb@example.com>',
          date: '2026-07-10T00:00:00Z',
        },
        {
          sha: 'c1',
          parents: [],
          refs: ['tag/v1'],
          title: 'First commit',
          author: 'seb <seb@example.com>',
          date: '2026-07-09T00:00:00Z',
        },
      ],
      truncated: false,
    } as LogResult);

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    // Le log doit avoir été chargé via gitClient.log
    const panel = wrapper.vm as unknown as { log: { value: unknown } };
    expect(panel.log.value).not.toBeNull();

    // L'interface expose renderGraph et graphContainer
    const exposed = wrapper.vm as unknown as {
      renderGraph: Function;
      graphContainer: HTMLElement | null;
      refresh: Function;
    };
    expect(exposed.renderGraph).toBeDefined();
    expect(exposed.graphContainer).toBeTruthy();
  });

  it('renderGraph ne plante pas lorsque log et container sont chargés', async () => {
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: true,
      files: [],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });
    mockClient.unpushed.mockResolvedValueOnce({
      branch: 'main',
      upstream: 'origin/main',
      commits: [],
      truncated: false,
    });
    mockClient.log.mockResolvedValueOnce({
      branch: 'main',
      commits: [],
      truncated: false,
    } as LogResult);

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const panel = wrapper.vm as unknown as {
      renderGraph: Function;
      log: { value: unknown };
    };
    // renderGraph devrait être appelable sans erreur
    expect(() => panel.renderGraph()).not.toThrow();
  });
});

describe('GitPanel.vue — boutons Diff', () => {
  it('cliquer Diff sur un fichier staged appelle openDiff(path, true)', async () => {
    const openDiffMock = vi.fn();
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'a.txt', state: 'modified', staged: true },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
          'open-diff': openDiffMock,
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const diffBtn = wrapper.findAll('button').find((b) => b.text() === 'Diff');
    expect(diffBtn).toBeDefined();
    await diffBtn!.trigger('click');

    expect(openDiffMock).toHaveBeenCalledWith('a.txt', true);
  });

  it('cliquer Diff sur un fichier unstaged appelle openDiff(path, undefined)', async () => {
    const openDiffMock = vi.fn();
    mockClient.status.mockResolvedValueOnce({
      branch: 'main',
      clean: false,
      files: [
        { path: 'b.txt', state: 'modified', staged: false },
      ],
    });
    mockClient.branches.mockResolvedValueOnce({
      current: 'main',
      merging: false,
      branches: [],
    });

    const wrapper = mount(GitPanel, {
      global: {
        provide: {
          'sandbox-name': 's',
          'open-diff': openDiffMock,
        } as Record<string, unknown>,
      },
    });

    await flushMicrotasks();

    const diffBtn = wrapper.findAll('button').find((b) => b.text() === 'Diff');
    expect(diffBtn).toBeDefined();
    await diffBtn!.trigger('click');

    expect(openDiffMock).toHaveBeenCalledWith('b.txt', undefined);
  });
});
