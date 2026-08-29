<script setup lang="ts">
import type { Component } from "vue";
import { computed, ref, watchEffect, onMounted } from "vue";
import type { ConfigNavGroup } from "./config-nav";

interface Props {
  groups: ConfigNavGroup[];
  screens: Record<string, Component>;
  activeScreen?: string;
}

const props = withDefaults(defineProps<Props>(), {});

const emit = defineEmits<{
  "update:activeScreen": [screenId: string];
  "nav-change": [
    payload: {
      groupId: string;
      screenId: string;
      groupLabel: string;
      screenLabel: string;
    },
  ];
}>();

const expandedGroupId = ref<string | null>(null);

// Première feuille de groups[0]
function firstLeaf(group: ConfigNavGroup): string {
  if (group.sub?.length) return group.sub[0].id;
  return group.id;
}

// État interne réel de l'écran sélectionné
const screenInternal = ref<string>(
  firstLeaf(props.groups[0] ?? { id: "", label: "", icon: "", accent: "" }),
);

// resolvedScreen = prop (contrôlé) ou état interne (non contrôlé)
const resolvedScreen = computed(() => {
  if (props.activeScreen !== undefined) return props.activeScreen;
  return screenInternal.value;
});

// activeGroup pour le highlight (dérivé de resolvedScreen)
const activeGroup = computed(() => {
  const screenId = resolvedScreen.value;
  for (const group of props.groups) {
    if (screenId === group.id) return group.id;
    if (group.sub?.some((s) => s.id === screenId)) {
      return group.id;
    }
  }
  return props.groups[0]?.id ?? null;
});

function onClickGroup(group: ConfigNavGroup) {
  if (resolvedScreen.value === group.id && group.sub) return;
  if (group.sub?.length) {
    if (expandedGroupId.value !== group.id) {
      expandedGroupId.value = group.id;
      selectScreen(group.sub[0].id);
    }
  } else {
    expandedGroupId.value = null;
    selectScreen(group.id);
  }
}

function onClickSub(_group: ConfigNavGroup, subId: string) {
  selectScreen(subId);
}

function selectScreen(screenId: string) {
  screenInternal.value = screenId;
  emit("update:activeScreen", screenId);
  emit("nav-change", resolveNav(screenId));
}

function resolveNav(screenId: string) {
  for (const group of props.groups) {
    if (screenId === group.id) {
      return {
        groupId: group.id,
        screenId,
        groupLabel: group.label,
        screenLabel: group.label,
      };
    }
    if (group.sub?.some((s) => s.id === screenId)) {
      const sub = group.sub.find((s) => s.id === screenId)!;
      return {
        groupId: group.id,
        screenId,
        groupLabel: group.label,
        screenLabel: sub.label,
      };
    }
  }
  // Écran non trouvé dans les groupes → première feuille
  const first = firstLeaf(props.groups[0]);
  return {
    groupId: props.groups[0].id,
    screenId,
    groupLabel: props.groups[0].label,
    screenLabel: first,
  };
}

// Synchroniser prop contrôlée → état interne
watchEffect(() => {
  if (props.activeScreen !== undefined) {
    screenInternal.value = props.activeScreen;
  } else if (screenInternal.value === undefined) {
    // Initialiser en non-contrôlé
    screenInternal.value = firstLeaf(props.groups[0]);
  }
});

// Émission nav-change au montage
onMounted(() => {
  emit("nav-change", resolveNav(resolvedScreen.value));
});
</script>

<template>
  <div class="settings">
    <nav class="nav" aria-label="Configuration">
      <template v-for="group in groups" :key="group.id">
        <button
          class="nav-item"
          :class="{ active: activeGroup === group.id }"
          :data-group="group.id"
          :style="{ '--accent': group.accent }"
          @click="onClickGroup(group)"
        >
          <span class="nav-icon">{{ group.icon }}</span>
          <span class="nav-label">{{ group.label }}</span>
          <template v-if="group.sub">
            <span
              class="nav-arrow"
              :class="{ expanded: expandedGroupId === group.id }"
              @click.stop
              >▼</span
            >
          </template>
        </button>
        <template v-if="group.sub && expandedGroupId === group.id">
          <button
            v-for="sub in group.sub"
            :key="sub.id"
            class="nav-sub-item"
            :class="{ active: resolvedScreen === sub.id }"
            :style="{ '--accent': group.accent }"
            @click="onClickSub(group, sub.id)"
          >
            {{ sub.label }}
          </button>
        </template>
      </template>
    </nav>
    <main class="panels">
      <div class="screen-wrap">
        <component
          v-if="resolvedScreen && screens[resolvedScreen]"
          :is="screens[resolvedScreen]"
        />
        <slot v-else name="pending" :screen-id="resolvedScreen">
          <div class="pending"><span class="pending-icon">🔜</span>À venir</div>
        </slot>
      </div>
    </main>
  </div>
</template>

<style scoped>
.settings {
  height: 100%;
  width: 100%;
  display: flex;
  background: #0c1420;
  color: #e6e9f0;
  font-size: 13px;
}
.nav {
  width: 240px;
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 24px 14px;
  border-right: 1px solid #1c1c2a;
}
.nav-item {
  appearance: none;
  border: none;
  border-left: 3px solid transparent;
  background: transparent;
  color: #9497a9;
  display: flex;
  align-items: center;
  gap: 10px;
  text-align: left;
  font: inherit;
  font-size: 13.5px;
  height: 38px;
  padding: 0 10px;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  width: 100%;
  font-family: inherit;
}
.nav-icon {
  width: 18px;
  text-align: center;
  font-size: 15px;
  color: var(--accent);
}
.nav-item:hover {
  background: #161d2c;
  color: white;
}
.nav-item.active {
  background: #161d2c;
  border-left-color: var(--accent);
  color: white;
  font-weight: 600;
}
.nav-arrow {
  margin-left: auto;
  font-size: 10px;
  color: #6a7185;
  transition: transform 0.15s;
}
.nav-arrow.expanded {
  transform: rotate(180deg);
}
.nav-sub-item {
  appearance: none;
  background: transparent;
  border: none;
  color: #6a7185;
  display: block;
  text-align: left;
  font: inherit;
  font-size: 12.5px;
  height: 32px;
  padding: 0 10px 0 42px;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  width: 100%;
  font-family: inherit;
}
.nav-sub-item:hover {
  background: #161d2c;
  color: white;
}
.nav-sub-item.active {
  color: white;
  font-weight: 600;
}

.panels {
  flex: 1;
  overflow-y: auto;
  padding: 48px 56px;
}
.screen-wrap {
  max-width: 760px;
}

.pending {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 100%;
  color: #6a7185;
  font-size: 14px;
}
.pending-icon {
  font-size: 22px;
}
</style>
