import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import type { ConfigRepo } from '../ports';
import { CONFIG_REPO_KEY } from './useConfigRepo';
import SkillsScreen from './SkillsScreen.vue';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

/** Fake repo `skills` avec un store en mémoire. */
function skillsRepo(initial: Array<{ name: string; description: string; body: string }> = []) {
  const store = new Map(initial.map((s) => [s.name, { ...s }]));
  const repo = {
    list: vi.fn(async () => [...store.values()].map((s) => ({ name: s.name, description: s.description }))),
    get: vi.fn(async (_d: string, name: string) => {
      const s = store.get(name);
      if (!s) throw new Error(`${name} introuvable`);
      return { ...s };
    }),
    create: vi.fn(async (_d: string, item: { name: string; description: string; body: string }) => {
      store.set(item.name, { ...item });
      return { ...item };
    }),
    update: vi.fn(async (_d: string, name: string, patch: Partial<{ description: string; body: string }>) => {
      const s = { ...store.get(name)!, ...patch };
      store.set(name, s);
      return s;
    }),
    remove: vi.fn(async (_d: string, name: string) => {
      store.delete(name);
    }),
  };
  return repo as unknown as ConfigRepo;
}

function mountWith(repo: ConfigRepo) {
  return mount(SkillsScreen, { global: { provide: { [CONFIG_REPO_KEY]: repo } } });
}

describe('SkillsScreen', () => {
  it('affiche les noms + descriptions', async () => {
    const w = mountWith(
      skillsRepo([
        { name: 'git-skill', description: 'Outils git', body: '' },
        { name: 'deploy-skill', description: 'Déploiement K8s', body: '' },
      ]),
    );
    await flushPromises();
    expect(w.text()).toContain('git-skill');
    expect(w.text()).toContain('deploy-skill');
    expect(w.text()).toContain('Outils git');
    expect(w.text()).toContain('Déploiement K8s');
  });

  it('état vide quand aucun skill', async () => {
    const w = mountWith(skillsRepo());
    await flushPromises();
    expect(w.text()).toContain('Aucun skill');
  });

  it('création : remplir la modale → create(item) → modale fermée → nouvelle ligne', async () => {
    const repo = skillsRepo([{ name: 'existing', description: 'x', body: '' }]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-create').trigger('click');
    await flushPromises();
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog).toBeTruthy();

    const set = (sel: string, val: string) => {
      const el = dialog.querySelector(sel) as HTMLInputElement | HTMLTextAreaElement;
      el.value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    set('input[aria-label="Nom du skill"]', 'my-skill');
    set('textarea[aria-label="Description"]', 'Ma description');
    set('textarea[aria-label="Body"]', '# body');

    await (dialog.querySelector('.btn-create') as HTMLElement).click();
    await flushPromises();

    expect(repo.create).toHaveBeenCalledWith('skills', {
      name: 'my-skill',
      description: 'Ma description',
      body: '# body',
    });
    expect((w.vm as unknown as { createModalOpen: boolean }).createModalOpen).toBe(false);
    expect(w.text()).toContain('my-skill');
  });

  it('édition : get(name) charge le body → save appelle update(name, patch)', async () => {
    const repo = skillsRepo([{ name: 'git-skill', description: 'old desc', body: '# old body' }]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-edit').trigger('click');
    await flushPromises();

    expect(repo.get).toHaveBeenCalledWith('skills', 'git-skill');
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.textContent).toContain('Modifier : git-skill');
    const desc = dialog.querySelector('textarea[aria-label="Description"]') as HTMLTextAreaElement;
    const body = dialog.querySelector('textarea[aria-label="Body"]') as HTMLTextAreaElement;
    expect(desc.value).toBe('old desc');
    expect(body.value).toBe('# old body');

    desc.value = 'updated-desc';
    desc.dispatchEvent(new Event('input', { bubbles: true }));
    body.value = '# new body';
    body.dispatchEvent(new Event('input', { bubbles: true }));

    await (dialog.querySelector('.btn-success') as HTMLElement).click();
    await flushPromises();

    expect(repo.update).toHaveBeenCalledWith('skills', 'git-skill', {
      description: 'updated-desc',
      body: '# new body',
    });
    expect((w.vm as unknown as { editModalOpen: boolean }).editModalOpen).toBe(false);
    expect(w.text()).toContain('updated-desc');
  });

  it('suppression : remove(name) → refetch → état vide', async () => {
    const repo = skillsRepo([{ name: 'to-delete', description: 'x', body: '' }]);
    const w = mountWith(repo);
    await flushPromises();

    await w.find('.btn-delete').trigger('click');
    await flushPromises();

    expect(repo.remove).toHaveBeenCalledWith('skills', 'to-delete');
    expect(w.text()).toContain('Aucun skill');
  });
});
