import { ref, type Ref } from 'vue';

export interface IdeActions {
  saveActiveFile?: () => void;
  closeActiveTab?: () => void;
  openExplorer?: () => void;
  newTerminal?: () => void;
  findInActiveFile?: () => void;
  replaceInActiveFile?: () => void;
}

/** Singleton partagé — pont entre le menu global (`MenuBar.vue`, monté une
 *  fois dans `App.vue`) et l'`IdeShell` actuellement ouvert pour une
 *  sandbox : `IdeShell`/`Editor` y enregistrent leurs handlers au montage
 *  (fusion, pas remplacement — plusieurs composants contribuent), le menu
 *  les invoque sans connaître ces composants. `activeConversationId` fait
 *  aussi office de signal pour la colonne assistant : elle ne doit exister
 *  que si une vraie session (conversation créée côté backend) est active. */
const ideActions = ref<IdeActions>({});
const activeConversationId: Ref<string | null> = ref(null);
const startingSession = ref(false);
const sessionError = ref<string | null>(null);

export function useIdeSession() {
  return { ideActions, activeConversationId, startingSession, sessionError };
}

/** Fusionne — n'écrase pas les handlers déjà enregistrés par un autre
 *  composant (ex. `Editor.vue` pour `saveActiveFile`, `IdeShell.vue` pour
 *  `closeActiveTab`). */
export function registerIdeActions(actions: IdeActions): void {
  ideActions.value = { ...ideActions.value, ...actions };
}

/** À appeler au démontage d'`IdeShell` (quitter la route sandbox) : plus de
 *  handlers valides, plus de session active à afficher. */
export function clearIdeActions(): void {
  ideActions.value = {};
  activeConversationId.value = null;
  sessionError.value = null;
}