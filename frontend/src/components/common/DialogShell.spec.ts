import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import { defineComponent, ref } from 'vue';
import DialogShell from './DialogShell.vue';
// Source du SFC : test de structure du style (le rendu réel n'est pas vérifié en jsdom).
import dialogShellSource from './DialogShell.vue?raw';

beforeEach(() => {
  document.body.querySelectorAll('[role="dialog"]').forEach((d) => d.remove());
});

// v-model impose un composant hôte : DialogShell seul n'a pas d'état d'ouverture propre.
function Host(initialOpen: boolean) {
  return defineComponent({
    components: { DialogShell },
    setup() {
      const open = ref(initialOpen);
      return { open };
    },
    template: `
      <DialogShell v-model:open="open" title="Créer un skill">
        <p>corps de la modale</p>
        <template #actions>
          <button class="btn btn-create">Créer</button>
        </template>
      </DialogShell>
    `,
  });
}

describe('DialogShell', () => {
  it('rendu fermé : aucun dialog dans le DOM', () => {
    mount(Host(false));
    expect(document.querySelector('[role="dialog"]')).toBeFalsy();
  });

  it('rendu ouvert : titre, contenu et slot actions visibles', async () => {
    mount(Host(true));
    await new Promise((r) => setTimeout(r, 0));
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog!.textContent).toContain('Créer un skill');
    expect(dialog!.textContent).toContain('corps de la modale');
    expect(dialog!.querySelector('.btn-create')).toBeTruthy();
  });

  it('sous md (767 px et moins) : dialogue plein écran, sans rayon, défilement interne', () => {
    const css = dialogShellSource.split('<style>')[1] ?? '';
    const media = css.match(/@media \(max-width: 767px\)\s*\{([\s\S]*?)\n\}/);
    expect(media).toBeTruthy();
    const block = media![1];
    expect(block).toMatch(/\[role='dialog'\]\s*\{/);
    expect(block).toMatch(/width:\s*100%/);
    expect(block).toMatch(/height:\s*100%/);
    expect(block).toMatch(/border-radius:\s*0/);
    expect(block).toMatch(/transform:\s*none/);
    // Défilement interne conservé (règle de base) : le contenu long ne déborde pas l'écran.
    expect(css).toMatch(/overflow-y:\s*auto/);
  });

  it('au-dessus de md : largeur maximale 480 px inchangée', () => {
    const css = dialogShellSource.split('<style>')[1] ?? '';
    expect(css).toMatch(/max-width:\s*480px/);
    expect(css).not.toMatch(/@media \(min-width/);
  });
});
