<script setup lang="ts">
import {
  MenubarRoot,
  MenubarMenu,
  MenubarTrigger,
  MenubarPortal,
  MenubarContent,
  MenubarItem,
  MenubarSeparator,
} from 'reka-ui';
import { useRoute, useRouter } from 'vue-router';
import { startAgentSession, useIdeSession } from '../composables/useIdeSession';

interface Item {
  label: string;
  shortcut?: string;
  sep?: false;
  action?:
    | 'toggle-settings'
    | 'save-file'
    | 'close-tab'
    | 'goto-project'
    | 'start-agent-session'
    | 'open-explorer'
    | 'new-terminal';
}
interface Sep {
  sep: true;
}

const router = useRouter();
const route = useRoute();
const { ideActions } = useIdeSession();

// Uniquement les entrées réellement câblées (un `action` géré par onSelect) —
// pas d'items décoratifs qui ne font rien au clic. Menus "Édition" et "Aide"
// supprimés en entier : aucune de leurs entrées n'a jamais eu de handler.
const menus: { label: string; items: (Item | Sep)[] }[] = [
  {
    label: 'Fichier',
    items: [
      { label: 'Enregistrer', shortcut: '⌘S', action: 'save-file' },
      { sep: true },
      { label: "Fermer l'onglet", shortcut: '⌘W', action: 'close-tab' },
      { sep: true },
      { label: 'Vers le projet', action: 'goto-project' },
    ],
  },
  {
    label: 'Affichage',
    items: [
      { label: 'Explorer', action: 'open-explorer' },
      { label: 'Nouveau terminal', shortcut: '⌃`', action: 'new-terminal' },
      { sep: true },
      { label: 'Configuration', shortcut: '⌘,', action: 'toggle-settings' },
    ],
  },
  {
    label: 'Exécution',
    items: [{ label: 'Nouvelle session agent', action: 'start-agent-session' }],
  },
];

function isSep(item: Item | Sep): item is Sep {
  return 'sep' in item && item.sep === true;
}

function onSelect(item: Item) {
  switch (item.action) {
    case 'toggle-settings':
      router.push('/settings');
      break;
    case 'save-file':
      ideActions.value.saveActiveFile?.();
      break;
    case 'close-tab':
      ideActions.value.closeActiveTab?.();
      break;
    case 'goto-project': {
      const projectName = route.params.projectName;
      if (typeof projectName === 'string') router.push(`/p/${projectName}`);
      break;
    }
    case 'start-agent-session':
      void startAgentSession();
      break;
    case 'open-explorer':
      ideActions.value.openExplorer?.();
      break;
    case 'new-terminal':
      ideActions.value.newTerminal?.();
      break;
    default:
      break;
  }
}
</script>

<template>
  <MenubarRoot class="menubar">
    <MenubarMenu v-for="menu in menus" :key="menu.label" :value="menu.label">
      <MenubarTrigger class="trigger">{{ menu.label }}</MenubarTrigger>
      <MenubarPortal>
        <MenubarContent class="content" :side-offset="2" align="start">
          <template v-for="(item, i) in menu.items" :key="i">
            <MenubarSeparator v-if="isSep(item)" class="separator" />
            <MenubarItem v-else class="item" @select="onSelect(item)">
              <span>{{ item.label }}</span>
              <span v-if="item.shortcut" class="shortcut">{{ item.shortcut }}</span>
            </MenubarItem>
          </template>
        </MenubarContent>
      </MenubarPortal>
    </MenubarMenu>
  </MenubarRoot>
</template>

<style scoped>
.menubar {
  display: flex;
  align-items: center;
  height: 100%;
}
.trigger {
  appearance: none;
  border: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.75);
  font: inherit;
  font-size: 12.5px;
  height: 24px;
  padding: 0 9px;
  border-radius: 4px;
  cursor: default;
}
.trigger:hover,
.trigger[data-highlighted] {
  background: rgba(255, 255, 255, 0.08);
  color: white;
}
.trigger[data-state='open'] {
  background: #2b2b4a;
  color: white;
}

</style>

<!--
  Reka UI's Menu*/Menubar* internals drop consumer `class` on some deeply
  nested layers (MenubarContent -> ... -> PopperContent chain): several
  intermediate components only forward their own declared props, not
  arbitrary attrs, so `class="content"` never reliably reaches the real
  DOM node — it rendered with no background at all (see-through dropdown).
  Styling against the library's own stable `data-*`/`role` hooks instead
  is unaffected by that forwarding chain, so this block is deliberately
  global (unscoped), not a workaround left in by mistake.
-->
<style>
[data-reka-menubar-content] {
  min-width: 240px;
  background-color: #1c1c2a;
  border: 1px solid #2b2b4a;
  border-radius: 5px;
  padding: 4px;
  box-shadow: 0 8px 24px -8px rgba(0, 0, 0, 0.6);
  font-size: 12.5px;
  z-index: 9999;
}
[data-reka-menubar-content] [role='menuitem'] {
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
[data-reka-menubar-content] [role='menuitem'][data-highlighted] {
  background: #5b1ecf;
  color: white;
}
[data-reka-menubar-content] .shortcut {
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.45);
}
[data-reka-menubar-content] [role='menuitem'][data-highlighted] .shortcut {
  color: rgba(255, 255, 255, 0.75);
}
[data-reka-menubar-content] [role='separator'] {
  height: 1px;
  margin: 4px 6px;
  background: #2b2b4a;
}
</style>
