<script setup lang="ts">
import {
  DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle,
} from 'reka-ui';

defineProps<{ title: string }>();
const open = defineModel<boolean>('open', { required: true });
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="dialog-overlay" />
      <DialogContent class="dialog-content" role="dialog">
        <DialogTitle class="dialog-title">{{ title }}</DialogTitle>
        <slot />
        <div class="dialog-actions">
          <slot name="actions" />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style>
/* Non scoped : DialogPortal téléporte hors de l'arborescence du composant
   (hors de <body> de #app), le CSS scoped de Vue ne l'atteindrait pas. */
.dialog-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 1000;
}

[role='dialog'] {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  z-index: 1001;
  background: #101828;
  border: 1px solid #1c1c2a;
  border-radius: 10px;
  padding: 24px 28px;
  max-width: 480px;
  max-height: 85vh;
  overflow-y: auto;
}

/* Sous md (768 px) : dialogue plein écran, défilement interne (spec WorkflowEditor, 360 px). */
@media (max-width: 767px) {
  [role='dialog'] {
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    max-width: 100%;
    max-height: 100%;
    transform: none;
    border: 0;
    border-radius: 0;
    box-sizing: border-box;
  }
}

.dialog-title {
  margin: 0 0 16px 0;
  font-size: 15px;
  font-weight: 600;
  color: #e6e9f0;
}

.dialog-actions {
  display: flex;
  gap: 12px;
  justify-content: flex-end;
  margin-top: 12px;
}
</style>
