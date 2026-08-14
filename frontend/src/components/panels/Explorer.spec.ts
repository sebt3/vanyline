import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import { ElTree } from 'element-plus';
import Explorer from './Explorer.vue';

function makeClient() {
  return {
    request: vi.fn().mockImplementation(
      async (op: string, params: Record<string, unknown>) => {
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

  it('client non prêt → placeholder, pas de requête', async () => {
    const nullClient = ref(null) as ReturnType<typeof ref>;

    const wrapper = mount(Explorer, {
      global: {
        provide: {
          'sandbox-fs': nullClient,
          'sandbox-name': 'foo',
          'open-file': vi.fn(),
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
