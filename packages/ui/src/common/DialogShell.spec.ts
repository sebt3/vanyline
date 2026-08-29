import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import { defineComponent, ref } from 'vue';
import DialogShell from './DialogShell.vue';

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
});