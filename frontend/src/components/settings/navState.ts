import { ref } from 'vue';

/** Sélection courante de SettingsView, lue par AppBreadcrumb pour le fil d'ariane
 *  de /settings. Labels = libellés d'affichage (pas les ids). */
export const activeNav = ref<{ groupLabel: string; screenLabel: string }>({
  groupLabel: 'Modèles',
  screenLabel: 'Fournisseurs LLM',
});
