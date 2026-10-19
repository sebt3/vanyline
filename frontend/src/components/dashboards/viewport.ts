import { onBeforeUnmount, ref, type Ref } from 'vue';

/** Seuil `md` (768 px) : en dessous, assistants en feuille plein écran et colonne unique. */
export const MD_MIN_WIDTH = 768;

/** Seuil `lg` (1024 px) : en dessous, l'IDE est désactivé (frontend.sdd). */
export const LG_MIN_WIDTH = 1024;

/** Largeur de fenêtre courante, mise à jour au redimensionnement. À appeler dans `setup`. */
export function useViewportWidth(): Ref<number> {
  const width = ref(window.innerWidth);
  const onResize = () => {
    width.value = window.innerWidth;
  };
  window.addEventListener('resize', onResize);
  onBeforeUnmount(() => window.removeEventListener('resize', onResize));
  return width;
}
