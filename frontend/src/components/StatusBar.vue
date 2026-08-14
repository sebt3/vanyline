<script setup lang="ts">
import AppBreadcrumb from './AppBreadcrumb.vue';

defineProps<{ workspace?: string; extended?: boolean }>();
</script>

<template>
  <div class="statusbar">
    <AppBreadcrumb />
    <span class="grow" />
    <template v-if="extended">
      <span class="item">⎇ main</span>
      <span class="item">{{ workspace }}</span>
      <span class="item">● connecté — strix-cluster</span>
      <span class="item">UTF-8</span>
    </template>
  </div>
</template>

<style scoped>
.statusbar {
  height: 22px;
  flex: none;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 0 10px;
  background: var(--dv-color-abyss-accent);
  color: white;
  font-size: 11px;
  font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
}
.grow { flex: 1; }
.item { opacity: 0.92; }
</style>
