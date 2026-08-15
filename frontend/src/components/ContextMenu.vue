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

defineProps<{ entries: ContextMenuEntry[] }>();

function isSep(entry: ContextMenuEntry): entry is ContextMenuSeparatorEntry {
  return 'sep' in entry && entry.sep === true;
}
function onSelect(entry: ContextMenuAction): void {
  entry.action();
}
</script>

<template>
  <ContextMenuRoot>
    <ContextMenuTrigger class="trigger">
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

<style scoped>
.trigger {
  appearance: none;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: default;
}
</style>

<!--
  Reka UI's Menu*/Menubar* internals drop consumer `class` on some deeply
  nested layers (ContextMenu -> ... -> PopperContent chain): several
  intermediate components only forward their own declared props, not
  arbitrary attrs, so `class="content"` never reliably reaches the real
  DOM node — it rendered with no background at all (see-through dropdown).
  Styling against the library's own stable `data-*`/`role` hooks instead
  is unaffected by that forwarding chain, so this block is deliberately
  global (unscoped), not a workaround left in by mistake.
-->
<style>
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
