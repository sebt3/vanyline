<script setup lang="ts">
defineProps<{ workspace: string }>();
</script>

<template>
  <div class="statusbar">
    <span class="item">⎇ main</span>
    <span class="item">{{ workspace }}</span>
    <span class="grow" />
    <span class="item">● connecté — strix-cluster</span>
    <span class="item">UTF-8</span>
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
