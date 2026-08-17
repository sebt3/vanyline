<script setup lang="ts">
import {
  ContextMenuRoot,
  ContextMenuTrigger,
  ContextMenuPortal,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from 'reka-ui';

export interface ContextMenuAction {
  label: string;
  shortcut?: string;
  action: () => void;
}
export interface ContextMenuSeparatorEntry {
  sep: true;
}
export type ContextMenuEntry = ContextMenuAction | ContextMenuSeparatorEntry;

defineProps<{ entries: ContextMenuEntry[]; fill?: boolean }>();

function isSep(entry: ContextMenuEntry): entry is ContextMenuSeparatorEntry {
  return 'sep' in entry && entry.sep === true;
}
function onSelect(entry: ContextMenuAction): void {
  entry.action();
}
</script>

<template>
  <ContextMenuRoot>
    <!--
      Jamais `as-child` : `Primitive`/`Slot` de reka-ui (asChild) CLONENT le
      vnode du slot et suppriment explicitement tout `ref` qu'il porte
      (`delete firstNonCommentChildren.props?.ref` dans Slot.js) pour poser
      le leur à la place — un `ref` du consommateur sur l'élément slotté
      (Editor.vue/Terminal.vue montent CodeMirror/xterm dessus) ne se résout
      alors jamais. Bug trouvé en usage réel : `view`/`term` se construisaient
      quand même (accepte un parent absent sans planter), mais rien n'était
      jamais attaché au DOM — aucune erreur, juste un panneau vide. `fill`
      rend un vrai wrapper (`div`, jamais cloné) plutôt que d'éviter le
      wrapper via asChild — le `ref` du consommateur reste donc valide.
    -->
    <ContextMenuTrigger :as="fill ? 'div' : 'span'" :class="['trigger', { 'trigger-fill': fill }]">
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent class="content" :side-offset="2" align="start">
        <template v-for="(entry, i) in entries" :key="i">
          <ContextMenuSeparator v-if="isSep(entry)" class="separator" />
          <ContextMenuItem v-else class="item" @select="onSelect(entry)">
            <span>{{ entry.label }}</span>
            <span v-if="entry.shortcut" class="shortcut">{{ entry.shortcut }}</span>
          </ContextMenuItem>
        </template>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>

<!--
  Reka UI's Menu*/Menubar* internals drop consumer `class` on some deeply
  nested layers (ContextMenu -> ... -> PopperContent chain): several
  intermediate components only forward their own declared props, not
  arbitrary attrs, so `class="content"` never reliably reaches the real
  DOM node — it rendered with no background at all (see-through dropdown).
  Styling against the library's own stable `data-*`/`role` hooks instead
  is unaffected by that forwarding chain, so this block is deliberately
  global (unscoped), not a workaround left in by mistake.

  Same problem for `.trigger`/`.trigger-fill` below, different mechanism:
  `ContextMenuTrigger` renders a `Fragment` with two root vnodes (an
  invisible `MenuAnchor` + the actual `Primitive`) — Vue's scoped-style
  `data-v-*` attribute is only auto-forwarded to a child component's root
  when it has a *single* unambiguous root element, so it never reliably
  lands on the real trigger div here. The class name itself does reach the
  DOM (confirmed in tests), but the *scoped* rule silently never matched it
  — `.trigger-fill`'s `height: 100%` never applied, `.editor-host`
  (percentage height against an effectively `auto`-height parent) resolved
  to `auto` too, so CodeMirror rendered its full document with nothing left
  to scroll (found in real usage: editor visible, no scrollbar, can't
  scroll — right after the DOM-attachment fix that made it visible at all).
-->
<style>
.trigger {
  appearance: none;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: default;
}
.trigger-fill {
  display: block;
  height: 100%;
  width: 100%;
}
[data-reka-menu-content] {
  min-width: 240px;
  background-color: #1c1c2a;
  border: 1px solid #2b2b4a;
  border-radius: 5px;
  padding: 4px;
  box-shadow: 0 8px 24px -8px rgba(0, 0, 0, 0.6);
  font-size: 12.5px;
  z-index: 9999;
}
[data-reka-menu-content] [role='menuitem'] {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  height: 26px;
  padding: 0 8px;
  border-radius: 3px;
  color: rgba(255, 255, 255, 0.85);
  cursor: default;
  outline: none;
}
[data-reka-menu-content] [role='menuitem'][data-highlighted] {
  background: #5b1ecf;
  color: white;
}
[data-reka-menu-content] .shortcut {
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.45);
}
[data-reka-menu-content] [role='menuitem'][data-highlighted] .shortcut {
  color: rgba(255, 255, 255, 0.75);
}
[data-reka-menu-content] [role='separator'] {
  height: 1px;
  margin: 4px 6px;
  background: #2b2b4a;
}
</style>
