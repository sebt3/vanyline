<template>
  <div class="skeleton-card">
    <div class="skeleton" />
    <div class="skeleton short" />
    <div class="skeleton short" />
  </div>
</template>

<style scoped>
.skeleton-card {
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

.skeleton {
  height: 16px;
  border-radius: 4px;
  background: linear-gradient(90deg, #1a2332 25%, #1f2b3d 50%, #1a2332 75%);
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}
.skeleton.short {
  width: 60%;
}
</style>