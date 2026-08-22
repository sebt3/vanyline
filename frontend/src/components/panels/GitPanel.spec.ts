import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import GitPanel from './GitPanel.vue';

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
    };
  }
  return { mockClient: makeMockGitClient() };
});

vi.mock('../../api/gitClient', () => ({
  gitClient: mockClient,
}));

describe('GitPanel.vue — statut, staging, commit', () => {
  beforeEach(() => {
    mockClient.status.mockReset();
    mockClient.branches.mockReset();
    mockClient.stage.mockReset();
    mockClient.unstage.mockReset();
    mockClient.commit.mockReset();
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
});