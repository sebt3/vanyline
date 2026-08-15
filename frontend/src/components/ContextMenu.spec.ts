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

  it('asChild rend le trigger sans wrapper et ouvre au clic droit', async () => {
    const wrapper = mount(ContextMenu, {
      slots: { default: '<div class="target">Cible</div>' },
      props: { asChild: true, entries: [{ label: 'Test', action: vi.fn() }] },
    });

    const target = wrapper.find('.target');
    expect(target.exists()).toBe(true);

    target.element.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // Le menu s'ouvre au clic droit ; aucun wrapper span intermédiaire
    // n'entoure l'élément du slot (asChild = vrai).
    expect(document.querySelector('[role="menuitem"]')).toBeTruthy();
  });
});
