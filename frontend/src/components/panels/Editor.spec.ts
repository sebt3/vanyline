import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ref } from 'vue';
import { mount } from '@vue/test-utils';
import Editor from './Editor.vue';

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

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('Editor.vue — contenu réel', () => {
  let client: ReturnType<typeof makeClient>;

  beforeEach(() => {
    client = makeClient();
  });

  it('lit le fichier au changement de openFilePath (raw: true)', async () => {
    const wrapper = mount(Editor, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'open-file-path': ref('src/main.py'),
        },
      },
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

  it('change de openFilePath relit le nouveau fichier', async () => {
    const openFileRef = ref('a.py');
    const c = makeClient();

    // mount() est un side-effect : initialise le composant + loadFile dans onMounted
    mount(Editor, {
      global: {
        provide: {
          'sandbox-fs': ref(c),
          'open-file-path': openFileRef,
        },
      },
    });

    await flushMicrotasks();

    client.request.mockClear();

    openFileRef.value = 'b.py';
    await flushMicrotasks();

    expect(c.request).toHaveBeenCalledWith('read', {
      path: 'b.py',
      raw: true,
    });
  });

  it('Ctrl+S écrit le contenu courant', async () => {
    const wrapper = mount(Editor, {
      global: {
        provide: {
          'sandbox-fs': ref(client),
          'open-file-path': ref('src/main.py'),
        },
      },
    });

    // 2 flush : onMounted(async read) + read async résolu
    await flushMicrotasks();
    await flushMicrotasks();

    const { save } = wrapper.vm as { save: () => void };
    save();

    await flushMicrotasks();

    // read appelé en premier (watch), write depuis save()
    expect(client.request).toHaveBeenCalledWith('write', {
      path: 'src/main.py',
      content: 'ligne1\nligne2\n',
    });
  });
});