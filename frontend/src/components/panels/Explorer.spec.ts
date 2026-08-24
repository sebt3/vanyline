import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import { ElTree } from 'element-plus';
import Explorer from './Explorer.vue';

/** Mock singleton pour `gitClient.status` — partagé entre tous les tests
 *  git states. Créé avec `vi.hoisted` pour éviter le TDZ. */
const { mockGitStatus } = vi.hoisted(() => ({
  mockGitStatus: vi.fn(),
}));

vi.mock('../../api/gitClient', () => ({
  gitClient: { status: mockGitStatus },
}));

vi.mock('../../api/gitClient', () => ({
  gitClient: { status: mockGitStatus },
}));

function makeClient() {
  const mockRequests: { resolved: boolean; rejects?: Error | null; called: Map<string, boolean> }[] = [];

  function resetMocks() {
    mockRequests.forEach((m) => {
      m.called.clear();
      if (m.rejects) {
        m.rejects.stack = undefined;
      }
    });
  }

  return {
    resetMocks,
    request: vi.fn().mockImplementation(
      async (op: string, params: Record<string, unknown>) => {
        // write, mkdir, rename, delete
        if (op === 'write') return { ok: true };
        if (op === 'mkdir') return { ok: true };
        if (op === 'rename') return { ok: true };
        if (op === 'delete') return { ok: true };
        if (op === 'list' && params.path === '.') {
          return { ok: true, entries: 'README.md\nsrc/\nworkflows/\n' };
        }
        if (op === 'list' && params.path === 'src') {
          return { ok: true, entries: 'main.py\njobs/\n' };
        }
        if (op === 'list' && params.path === 'src/jobs') {
          return { ok: true, entries: 'sync_library.py\n' };
        }
        return { ok: false, error: 'unknown' };
      },
    ),
  };
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('Explorer.vue — arbre réel', () => {
  let client: ReturnType<typeof makeClient>;
  let openFileSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    client = makeClient();
    openFileSpy = vi.fn();
  });

  it('charge la racine au montage et affiche les entrées', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': openFileSpy,
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    // le client est fourni → pas de placeholder
    expect(wrapper.find('.empty').exists()).toBe(false);

    // loadNode, exposée via defineExpose, appelée avec la racine
    const { loadNode } = wrapper.vm as {
      loadNode: (
        node: { data: unknown },
        resolve: (d: unknown[]) => void,
        reject: (e: Error) => void,
      ) => Promise<void>;
      parseEntries: (p: string, t: string) => unknown[];
    };
    const rootResolve = vi.fn();
    const rootReject = vi.fn();

    await loadNode(
      { data: { id: '.', label: 'foo', path: '.', leaf: false, children: [] } },
      rootResolve,
      rootReject,
    );

    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
    const resolved = rootResolve.mock.calls[0][0];
    expect(resolved).toHaveLength(3);
    expect(resolved.map((e: { label: string }) => e.label)).toContain('README.md');
    expect(resolved.map((e: { label: string }) => e.label)).toContain('src');
    expect(resolved.map((e: { label: string }) => e.label)).toContain('workflows');
  });

  it('déplie un dossier et charge paresseusement ses enfants', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(makeClient()),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    // loadNode, exposée via defineExpose, pour le nœud 'src'
    const { loadNode } = wrapper.vm as {
      loadNode: (
        node: { data: unknown },
        resolve: (d: unknown[]) => void,
        reject: (e: Error) => void,
      ) => Promise<void>;
    };

    const resolve = vi.fn();
    const reject = vi.fn();

    await loadNode(
      { data: { id: 'src', label: 'src', path: 'src', leaf: false, children: [] } },
      resolve,
      reject,
    );

    const resolveArgs = resolve.mock.calls[0][0];
    expect(resolveArgs).toHaveLength(2);
    const mainPy = resolveArgs.find((n: { label: string }) => n.label === 'main.py') as {
      id: string;
      path: string;
      leaf: boolean;
    };
    const jobs = resolveArgs.find((n: { label: string }) => n.label === 'jobs') as {
      id: string;
      leaf: boolean;
    };
    expect(mainPy).toBeDefined();
    expect(mainPy.id).toBe('src/main.py');
    expect(mainPy.leaf).toBe(true);
    expect(jobs).toBeDefined();
    expect(jobs.id).toBe('src/jobs');
    expect(jobs.leaf).toBe(false);
  });

  it('clic sur un fichier invoque open-file avec le chemin relatif', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(makeClient()),
          'sandbox-name': 'foo',
          'open-file': openFileSpy,
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    // onNodeClick, exposée via defineExpose
    const { onNodeClick } = wrapper.vm as { onNodeClick: (...args: unknown[]) => void };

    onNodeClick({ id: 'README.md', label: 'README.md', path: 'README.md', leaf: true });
    expect(openFileSpy).toHaveBeenCalledWith('README.md');

    onNodeClick({ id: 'src/main.py', label: 'main.py', path: 'src/main.py', leaf: true });
    expect(openFileSpy).toHaveBeenCalledWith('src/main.py');

    // un dossier ne déclenche pas openFile (le spy a 2 appels précédents, on vérifie
    // que le dossier n'en ajoute pas un 3e)
    const appelsAvant = openFileSpy.mock.calls.length;
    onNodeClick({ id: 'src', label: 'src', path: 'src', leaf: false });
    expect(openFileSpy.mock.calls.length).toBe(appelsAvant);
  });

  it("charge la racine automatiquement (sans appel manuel a loadNode) via l'auto-expand d'el-tree", async () => {
    // Régression : un arbre `lazy` (element-plus) appelle `load` une
    // première fois pour sa propre racine invisible avant celle de nos
    // données — `node.data` est alors le tableau `treeData` lui-même, pas
    // un FsNode. Sans le gérer, `data.path` valait `undefined`, le serveur
    // répondait `{"error":"missing path"}` et l'arbre affichait "No Data"
    // sans le moindre signal d'erreur visible. Ce test n'appelle PAS
    // loadNode manuellement — il vérifie le déclenchement automatique réel.
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': openFileSpy,
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
    expect(wrapper.text()).toContain('README.md');
    expect(wrapper.text()).toContain('src');
  });

  it('une icône est rendue par nœud de l\'arbre', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': openFileSpy,
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    // Chaque `.label` contient un `.file-icon` (svg d'Element Plus Icon)
    // 4 labels : "foo" (racine), README.md, src, workflows
    const labels = wrapper.findAll('.label');
    expect(labels).toHaveLength(4);

    for (const label of labels) {
      expect(label.find('.file-icon').exists()).toBe(true);
    }
    // Le dossier `src` est rendu par folderIcon (Folder) dont le tag est "el-icon"
    // — vérifier par l'absence de distinction, les icônes sont dans chaque `.label`.
  });

  it('client non prêt → placeholder, pas de requête', async () => {
    const nullClient = ref(null) as ReturnType<typeof ref>;

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': nullClient,
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    // aucune requête n'est envoyée
    // (le client n'est pas fourni, donc loadNode n'est pas invoqué)
    expect(wrapper.text()).toContain('Connexion à la sandbox');
    expect(wrapper.find('.empty').text()).toBe('Connexion à la sandbox…');
  });
});

// ---------- Tests CRUD ----------

describe('Explorer.vue — CRUD arbre', () => {
  let client: ReturnType<typeof makeClient>;
  let closeFileSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    client = makeClient();
    closeFileSpy = vi.fn();
    vi.stubGlobal('prompt', vi.fn());
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    client.resetMocks();
  });

  it('nouveau fichier envoie write avec contenu vide et rafraîchit', async () => {
    vi.mocked(window.prompt).mockReturnValue('new.txt');

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { createFile } = wrapper.vm as { createFile: (p: string) => void };
    createFile('src');

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('write', { path: 'src/new.txt', content: '' });
    // refresh → requête list '.'
    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
  });

  it('nouveau dossier envoie mkdir et rafraîchit', async () => {
    vi.mocked(window.prompt).mockReturnValue('d');

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { createDir } = wrapper.vm as { createDir: (p: string) => void };
    createDir('src');

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('mkdir', { path: 'src/d' });
    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
  });

  it('renommer envoie rename, ferme l\'onglet du fichier et rafraîchit', async () => {
    vi.mocked(window.prompt).mockReturnValue('renamed.txt');

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { renameNode } = wrapper.vm as { renameNode: (n: unknown) => void };
    renameNode({ id: 'src/main.py', label: 'main.py', path: 'src/main.py', leaf: true });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('rename', {
      path: 'src/main.py',
      to: 'src/renamed.txt',
    });
    expect(closeFileSpy).toHaveBeenCalledWith('src/main.py');
    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
  });

  it('renommer un dossier n\'appelle pas close-file', async () => {
    vi.mocked(window.prompt).mockReturnValue('lib');

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { renameNode } = wrapper.vm as { renameNode: (n: unknown) => void };
    renameNode({ id: 'src', label: 'src', path: 'src', leaf: false });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('rename', {
      path: 'src',
      to: 'lib',
    });
    expect(closeFileSpy).not.toHaveBeenCalled();
  });

  it('supprimer envoie delete, ferme l\'onglet si fichier ouvert et rafraîchit', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { deleteNode } = wrapper.vm as { deleteNode: (n: unknown) => void };
    deleteNode({ path: 'src/main.py', label: 'main.py', leaf: true });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('delete', { path: 'src/main.py' });
    expect(closeFileSpy).toHaveBeenCalledWith('src/main.py');
    expect(client.request).toHaveBeenCalledWith('list', { path: '.' });
  });

  it('échec de suppression affiche une erreur', async () => {
    // Remplacer request pour que delete retourne une erreur
    client.request = vi.fn().mockImplementation(
      async (op: string, params: Record<string, unknown>) => {
        if (op === 'delete' && params.path === 'src/jobs') {
          return { ok: false, error: 'directory is not empty' };
        }
        if (op === 'write') return { ok: true };
        if (op === 'mkdir') return { ok: true };
        if (op === 'rename') return { ok: true };
        if (op === 'list' && params.path === '.') {
          return { ok: true, entries: 'README.md\nsrc/\nworkflows/\n' };
        }
        return { ok: false, error: 'unknown' };
      },
    );

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { deleteNode } = wrapper.vm as { deleteNode: (n: unknown) => void };
    deleteNode({ path: 'src/jobs', label: 'jobs', leaf: false });

    await flushMicrotasks();
    await flushMicrotasks();

    // delete a été appelé, pas de refresh (list '.') après erreur
    expect(client.request).toHaveBeenCalledWith('delete', { path: 'src/jobs' });
    const deleteCallIndex = client.request.mock.calls.findIndex(
      (c) => c[0] === 'delete' && c[1]?.path === 'src/jobs',
    );
    // Aucune requête list '.' après la suppression (refresh seulement en cas de succès)
    const listAfterDelete = client.request.mock.calls.slice(deleteCallIndex + 1).some(
      (c) => c[0] === 'list',
    );
    expect(listAfterDelete).toBe(false);
    // bannière d'erreur visible
    expect(wrapper.text()).toContain('Suppression impossible');
    expect(wrapper.text()).toContain('directory is not empty');
  });

  it('annulation du prompt n\'envoie aucune requête', async () => {
    vi.mocked(window.prompt).mockReturnValue(null);

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const callCountBefore = client.request.mock.calls.length;

    const { createFile } = wrapper.vm as { createFile: (p: string) => void };
    createFile('src');

    await flushMicrotasks();

    // Aucune requête supplémentaire après l'annulation
    expect(client.request.mock.calls.length).toBe(callCountBefore);
  });

  it('annulation de la confirmation de suppression n\'envoie aucune requête', async () => {
    vi.mocked(window.confirm).mockReturnValue(false);

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': closeFileSpy,
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const callCountBefore = client.request.mock.calls.length;

    const { deleteNode } = wrapper.vm as { deleteNode: (n: unknown) => void };
    deleteNode({ path: 'src/main.py', label: 'main.py', leaf: true });

    await flushMicrotasks();

    expect(client.request.mock.calls.length).toBe(callCountBefore);
    expect(closeFileSpy).not.toHaveBeenCalled();
  });

  it('entriesForNode selon le type de nœud', async () => {
    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();

    const { entriesForNode } = wrapper.vm as { entriesForNode: (n: unknown) => unknown[] };

    // racine { path: '.', leaf: false } → Nouveau fichier, Nouveau dossier
    const rootEntries = entriesForNode({ id: '.', label: 'foo', path: '.', leaf: false });
    expect(rootEntries).toHaveLength(2);
    expect((rootEntries[0] as { label?: string }).label).toBe('Nouveau fichier');
    expect((rootEntries[1] as { label?: string }).label).toBe('Nouveau dossier');

    // fichier { path: 'a.py', leaf: true } → séparateur + copie de chemins + séparateur + Renommer + Supprimer
    const fileEntries = entriesForNode({ id: 'a.py', label: 'a.py', path: 'a.py', leaf: true });
    expect(fileEntries).toHaveLength(6);
    expect((fileEntries[0] as { sep?: boolean }).sep).toBe(true);
    expect((fileEntries[1] as { label?: string }).label).toBe('Copier le chemin relatif');
    expect((fileEntries[2] as { label?: string }).label).toBe('Copier le chemin absolu');
    expect((fileEntries[3] as { sep?: boolean }).sep).toBe(true);
    expect((fileEntries[4] as { label?: string }).label).toBe('Renommer');
    expect((fileEntries[5] as { label?: string }).label).toBe('Supprimer');

    // dossier { path: 'src', leaf: false } → Nouveau fichier, Nouveau dossier, séparateur,
    // copie de chemins, séparateur, Renommer, Supprimer
    const dirEntries = entriesForNode({ id: 'src', label: 'src', path: 'src', leaf: false });
    expect(dirEntries).toHaveLength(8);
    expect((dirEntries[0] as { label?: string }).label).toBe('Nouveau fichier');
    expect((dirEntries[1] as { label?: string }).label).toBe('Nouveau dossier');
    expect((dirEntries[2] as { sep?: boolean }).sep).toBe(true);
    expect((dirEntries[3] as { label?: string }).label).toBe('Copier le chemin relatif');
    expect((dirEntries[4] as { label?: string }).label).toBe('Copier le chemin absolu');
    expect((dirEntries[5] as { sep?: boolean }).sep).toBe(true);
    expect((dirEntries[6] as { label?: string }).label).toBe('Renommer');
    expect((dirEntries[7] as { label?: string }).label).toBe('Supprimer');
  });

  it('copier le chemin relatif écrit le chemin dans le presse-papiers', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });
    await flushMicrotasks();

    const { copyRelativePath } = wrapper.vm as { copyRelativePath: (n: unknown) => void };
    copyRelativePath({ id: 'src/a.py', label: 'a.py', path: 'src/a.py', leaf: true });
    await flushMicrotasks();

    expect(writeText).toHaveBeenCalledWith('src/a.py');
    expect(client.request).not.toHaveBeenCalledWith('root', {});
  });

  it('copier le chemin absolu appelle l\'op root et écrit le chemin joint', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    client.request.mockImplementation(async (op: string, params: Record<string, unknown>) => {
      if (op === 'root') return { ok: true, root: '/home/vanyline/workspace' };
      if (op === 'list' && params.path === '.') {
        return { ok: true, entries: 'README.md\nsrc/\nworkflows/\n' };
      }
      return { ok: false, error: 'unknown' };
    });

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });
    await flushMicrotasks();

    const { copyAbsolutePath } = wrapper.vm as { copyAbsolutePath: (n: unknown) => void };
    copyAbsolutePath({ id: 'src/a.py', label: 'a.py', path: 'src/a.py', leaf: true });
    await flushMicrotasks();
    await flushMicrotasks();

    expect(client.request).toHaveBeenCalledWith('root', {});
    expect(writeText).toHaveBeenCalledWith('/home/vanyline/workspace/src/a.py');
  });
});

// ---------- Tests git states ----------

describe('Explorer.vue — badges d\'état git', () => {
  let client: ReturnType<typeof makeClient>;

  beforeEach(() => {
    client = makeClient();
    mockGitStatus.mockResolvedValue({ branch: 'main', files: [], clean: true });
  });

  afterEach(() => {
    mockGitStatus.mockReset();
    mockGitStatus.mockResolvedValue({ branch: 'main', files: [], clean: true });
    client.resetMocks();
  });

  it('affiche un badge conflit pour un fichier conflicted', async () => {
    mockGitStatus.mockResolvedValueOnce({
      branch: 'main',
      files: [
        { path: 'README.md', state: 'conflicted', staged: true },
      ],
      clean: false,
    });

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();
    await flushMicrotasks();

    expect(wrapper.text()).toContain('conflit');
    const badge = wrapper.find('.git-state.conflicted');
    expect(badge.exists()).toBe(true);
    expect(badge.text()).toBe('conflit');
    // Vérifier aussi que stateOf retourne bien 'conflicted'
    const { stateOf } = wrapper.vm as { stateOf: (p: string) => string | undefined };
    expect(stateOf('README.md')).toBe('conflicted');
  });

  it('affiche un badge pour un fichier modifié', async () => {
    mockGitStatus.mockResolvedValueOnce({
      branch: 'main',
      files: [
        { path: 'README.md', state: 'modified', staged: false },
      ],
      clean: false,
    });

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();
    await flushMicrotasks();

    const { stateOf, stateClass, stateLabel: stateLabelFn } = wrapper.vm as {
      stateOf: (p: string) => string | undefined;
      stateClass: (p: string) => string;
      stateLabel: (p: string) => string;
    };
    expect(stateOf('README.md')).toBe('modified');
    expect(stateClass('README.md')).toBe('changed');
    expect(stateLabelFn('README.md')).toBe('modified');

    // Le badge est présent (le label-text ne contient pas de texte git,
    // mais le badge .git-state.changed oui)
    const badge = wrapper.find('.git-state.changed');
    expect(badge.exists()).toBe(true);
  });

  it('échec du statut git laisse aucun badge sans planter', async () => {
    mockGitStatus.mockRejectedValueOnce(new Error('network error'));

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
          'close-file': vi.fn(),
        } as Record<string, unknown>,
        components: { ElTree },
      },
      attachTo: document.body,
    });

    await flushMicrotasks();
    await flushMicrotasks();

    const { fileStates } = wrapper.vm as { fileStates: Map<string, string> };
    expect(fileStates.size).toBe(0);
    expect(wrapper.findAll('.git-state').length).toBe(0);
  });
});
