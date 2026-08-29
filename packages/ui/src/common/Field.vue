<script setup lang="ts">
defineProps<{ label: string; topAlign?: boolean }>();
</script>

<template>
  <label class="field" :class="{ 'field--top': topAlign }">
    <span class="field-label">{{ label }}</span>
    <slot />
  </label>
</template>

<style scoped>
/* .field-input (appliqué au contrôle passé en slot) vit dans style.css :
   authoré dans le template de l'appelant, il ne serait pas atteint par le
   CSS scoped de ce composant. */
.field {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 8px 12px;
  align-items: center;
  margin-bottom: 12px;
}
.field.field--top {
  align-items: start;
}

.field-label {
  font-size: 12px;
  font-weight: 600;
  color: #6a7185;
  text-transform: uppercase;
}
.field--top .field-label {
  padding-top: 6px;
}
</style>