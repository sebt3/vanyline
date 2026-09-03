<script setup lang="ts">
import type { ConfigEntrySource } from '../ports';

defineProps<{ source?: ConfigEntrySource }>();
</script>

<template>
  <span v-if="source" class="badge-source" :class="`badge-source-${source}`" data-testid="source-badge">
    {{ source }}
  </span>
</template>

<style scoped>
/* Forme alignée sur `.badge-default` (LlmProvidersScreen). Badge de couche
   en lecture seule : opacité moindre, il décore — ne filtre ni n'édite rien. */
.badge-source {
  display: inline-block;
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 9999px;
  opacity: 0.75;
}

.badge-source-workspace {
  background: #4c90f022;
  color: #4c90f0;
}

.badge-source-global {
  background: #e0a83d22;
  color: #e0a83d;
}
</style>
