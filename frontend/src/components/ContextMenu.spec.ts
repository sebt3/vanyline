import { defineComponent } from 'vue';
import { mount } from '@vue/test-utils';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ContextMenu, { type ContextMenuEntry, type ContextMenuAction } from './ContextMenu.vue';

/** Ouvre le menu contextuel en dispatchant un `contextmenu` sur un élément du slot,
 *  puis effectue l'action sur un item du menu. */
async function openContextMenuAndClick(
  wrapper: ReturnType<typeof mount>,
  itemLabel: string,
) {
  // Le trigger du menu contextuel est dans le DOM ; on dispatche sur un élément
  // du slot (il bulle jusqu'au trigger).
  const target = wrapper.find('.target');
  expect(target.exists()).toBe(true);
  target.element.dispatchEvent(
    new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
  );
  await wrapper.vm.$nextTick();
  await new Promise((r) => setTimeout(r, 0));

  const items = [...document.querySelectorAll('[role="menuitem"]')];
  const item = items.find((el) => el.textContent?.includes(itemLabel));
  expect(item).toBeTruthy();
  item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await wrapper.vm.$nextTick();
  await new Promise((r) => setTimeout(r, 0));
}

describe('ContextMenu', () => {
  afterEach(() => {
    // Nettoyer le body après chaque test (Portal téléporte le menu dedans).
    document.body.innerHTML = '';
  });

  it('clic droit ouvre le menu et le clic sur un item appelle son action', async () => {
    const action = vi.fn();
    const action2 = vi.fn();

    const entries: ContextMenuAction[] = [
      { label: 'Copier', action },
      { label: 'Renommer', action: action2 },
    ];

    const wrapper = mount(ContextMenu, {
      slots: { default: '<span class="target">Cible</span>' },
      props: { entries },
    });

    await openContextMenuAndClick(wrapper, 'Copier');

    expect(action).toHaveBeenCalledTimes(1);
    expect(action2).toHaveBeenCalledTimes(0);
  });

  it('séparateur rendu avec role="separator"', async () => {
    const entries: ContextMenuEntry[] = [
      { label: 'Copier', action: vi.fn() },
      { sep: true },
    ];

    const wrapper = mount(ContextMenu, {
      slots: { default: '<span class="target">Cible</span>' },
      props: { entries },
    });

    const target = wrapper.find('.target');
    expect(target.exists()).toBe(true);
    target.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="separator"]')).toBeTruthy();
  });

  it('shortcut affiché dans l\'item', async () => {
    const entries: ContextMenuAction[] = [
      { label: 'Copier', shortcut: '⌘C', action: vi.fn() },
    ];

    const wrapper = mount(ContextMenu, {
      slots: { default: '<span class="target">Cible</span>' },
      props: { entries },
    });

    const target = wrapper.find('.target');
    expect(target.exists()).toBe(true);
    target.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const item = items.find((el) => el.textContent?.includes('Copier'));
    expect(item).toBeTruthy();
    expect(item!.textContent).toContain('⌘C');
  });

  it('fill rend un wrapper div rempli plutôt qu\'un span inline', async () => {
    const wrapper = mount(ContextMenu, {
      slots: { default: '<div class="target">Cible</div>' },
      props: { fill: true, entries: [{ label: 'Test', action: vi.fn() }] },
    });

    const trigger = wrapper.find('.trigger');
    expect(trigger.exists()).toBe(true);
    expect(trigger.element.tagName).toBe('DIV');
    expect(trigger.classes()).toContain('trigger-fill');

    trigger.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(document.querySelector('[role="menuitem"]')).toBeTruthy();
  });

  it('un ref posé sur l\'élément slotté reste résolu avec fill (régression : asChild de reka-ui le supprimait silencieusement)', async () => {
    // Reproduit exactement le pattern Editor.vue/Terminal.vue : un
    // template ref sur l'élément passé dans le slot, utilisé au montage
    // pour y attacher une librairie tierce (CodeMirror/xterm). Avec l'ancien
    // `as-child`, reka-ui supprimait ce ref (Slot.js : `delete
    // firstNonCommentChildren.props?.ref`) — la lib se construisait quand
    // même (parent undefined accepté sans erreur) mais rien n'était jamais
    // attaché au DOM, sans aucune erreur visible.
    const Host = defineComponent({
      components: { ContextMenu },
      template: `
        <ContextMenu :entries="[]" fill>
          <div ref="host" class="host-target"></div>
        </ContextMenu>
      `,
      data() {
        return { mountedRef: null as HTMLElement | null };
      },
      mounted() {
        this.mountedRef = this.$refs.host as HTMLElement | null;
      },
    });

    const wrapper = mount(Host);

    expect((wrapper.vm as unknown as { mountedRef: HTMLElement | null }).mountedRef).not.toBeNull();
    expect(
      (wrapper.vm as unknown as { mountedRef: HTMLElement | null }).mountedRef,
    ).toBe(wrapper.find('.host-target').element);
  });
});
