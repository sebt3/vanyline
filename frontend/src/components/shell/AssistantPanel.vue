<script setup lang="ts">
// Contrat : ./shell.sdd. Enveloppe le ContextChat fourni par l'écran appelant.
defineProps<{ open: boolean; title: string }>();
const emit = defineEmits<{ close: [] }>();
</script>

<template>
  <aside v-if="open" class="assistant-panel" role="complementary" :aria-label="title">
    <header class="assistant-panel-head">
      <span class="assistant-panel-title">{{ title }}</span>
      <button type="button" class="assistant-panel-close" aria-label="Fermer" @click="emit('close')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true" focusable="false">
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </header>
    <div class="assistant-panel-body">
      <slot />
    </div>
  </aside>
</template>

<style scoped>
/* Colonne de 340 px à partir de md (768 px) ; feuille plein écran en dessous, au-dessus
   du contenu, la barre d'onglets restant accessible. */
.assistant-panel {
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--surface);
  color: var(--text);
  font-family: var(--font-sans);
  font-size: 13px;
}
.assistant-panel-head {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-height: var(--ctl-h);
  padding: 4px 8px 4px 12px;
  border-bottom: 1px solid var(--border);
}
.assistant-panel-title {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.assistant-panel-close {
  appearance: none;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: var(--ctl-h);
  height: var(--ctl-h);
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
}
.assistant-panel-close:hover {
  color: var(--text);
  background: var(--surface-2);
}
.assistant-panel-close:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.assistant-panel-close svg {
  width: 16px;
  height: 16px;
}
/* Le slot occupe le reste de la colonne ; le contenu défile en interne. */
.assistant-panel-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

@media (max-width: 767px) {
  .assistant-panel {
    position: fixed;
    inset: 0 0 var(--nav-h) 0;
    z-index: 20;
  }
}
@media (min-width: 768px) {
  .assistant-panel {
    width: 340px;
    flex: none;
    border-left: 1px solid var(--border);
  }
}
</style>
