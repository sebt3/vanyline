import type { SerializedDockview } from 'dockview-core';

/** Une clé par sandbox — deux sandboxes peuvent légitimement avoir des
 *  répartitions de panels différentes. */
export function layoutStorageKey(sandboxName: string): string {
  return `vanyline.ide.layout.${sandboxName}`;
}

/** `null` si rien de sauvegardé, ou si le JSON stocké est invalide (storage
 *  corrompu/format d'une version antérieure) — dans ce cas l'appelant doit
 *  retomber sur le layout par défaut plutôt que planter. */
export function loadLayout(sandboxName: string): SerializedDockview | null {
  const raw = localStorage.getItem(layoutStorageKey(sandboxName));
  if (!raw) return null;
  try {
    return JSON.parse(raw) as SerializedDockview;
  } catch {
    return null;
  }
}

export function saveLayout(sandboxName: string, layout: SerializedDockview): void {
  localStorage.setItem(layoutStorageKey(sandboxName), JSON.stringify(layout));
}

/** Anti-rebond : `onDidLayoutChange` se déclenche à chaque frame pendant un
 *  drag (resize/déplacement de panel) — écrire à chaque frame serait un
 *  gaspillage sans intérêt pour de la persistance. */
export function debounce<Args extends unknown[]>(
  fn: (...args: Args) => void,
  delayMs: number,
): (...args: Args) => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (...args: Args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), delayMs);
  };
}
