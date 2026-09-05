/** Vrai si le nom de base de `path` est un nom de fichier Docker/Compose-style :
 *  `Dockerfile`, `Dockerfile.*` (ex. `Dockerfile.dev`), `*.dockerfile`
 *  (ex. `app.dockerfile`), `Containerfile`, `Containerfile.*` — le tout insensible
 *  à la casse. Chemin relatif POSIX (dernier segment après le dernier `/`).
 *  Chaîne vide → false. */
export function dockerfileName(path: string): boolean {
  if (!path) return false;
  const idx = path.lastIndexOf('/');
  const base = (idx === -1 ? path : path.slice(idx + 1)).toLowerCase();
  if (base === 'dockerfile' || base === 'containerfile') return true;
  if (base.startsWith('dockerfile.') || base.startsWith('containerfile.')) return true;
  return base.endsWith('.dockerfile');
}
