<script setup lang="ts">
defineProps<{ projectName: string }>();
</script>

<template>
  <div class="dashboard">
    <h1>Sandboxes de {{ projectName }}</h1>
    <p>Aucune sandbox.</p>
  </div>
</template>

<style scoped>
.dashboard {
  background: #0c1420;
  color: #e6e9f0;
  padding: 48px 56px;
}
h1 {
  font-size: 18px;
  font-weight: 600;
  margin: 0 0 8px 0;
}
</style>