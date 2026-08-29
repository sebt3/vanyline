<script setup lang="ts">
defineProps<{ message: string }>();
</script>

<template>
  <div class="card" role="alert">
    <p class="error-text">{{ message }}</p>
  </div>
</template>

<style scoped>
.card {
  display: grid;
  grid-template-columns: 1fr;
  gap: 22px 28px;
  max-width: 760px;
  padding: 28px 32px;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  margin-bottom: 12px;
}

.error-text {
  color: #e85d5d;
  font-size: 13px;
  margin: 0;
}
</style>