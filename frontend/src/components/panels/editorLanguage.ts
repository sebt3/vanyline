import type { Extension } from '@codemirror/state';
import { StreamLanguage } from '@codemirror/language';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { markdown } from '@codemirror/lang-markdown';
import { python } from '@codemirror/lang-python';
import { rust } from '@codemirror/lang-rust';
import { yaml } from '@codemirror/lang-yaml';
import { toml } from '@codemirror/legacy-modes/mode/toml';

/** Support MVP : ts/js/rust (langages produits) + json/markdown/toml/yaml
 *  (config/doc courants dans ces mêmes projets) + python (déjà présent,
 *  gardé — le support natif viendra plus tard). Extension de fichier →
 *  extension CodeMirror ; chemin sans extension connue → aucun langage
 *  (coloration désactivée, pas de plantage). */
const byExtension: Record<string, () => Extension> = {
  ts: () => javascript({ typescript: true }),
  tsx: () => javascript({ typescript: true, jsx: true }),
  js: () => javascript(),
  jsx: () => javascript({ jsx: true }),
  mjs: () => javascript(),
  cjs: () => javascript(),
  rs: () => rust(),
  json: () => json(),
  md: () => markdown(),
  markdown: () => markdown(),
  yaml: () => yaml(),
  yml: () => yaml(),
  toml: () => StreamLanguage.define(toml),
  py: () => python(),
};

/** Mapping chemin → (toolchain, languageId LSP) — identique à `toolchain_for_path`
 *  de la sandbox (task-04). `null` si l'extension n'est pas couverte (pas de LSP,
 *  mode dégradé). */
export function lspToolchainForPath(
  path: string | null,
): { toolchain: string; languageId: string } | null {
  if (!path) return null;
  const ext = path.split('.').pop()?.toLowerCase();
  if (!ext) return null;
  switch (ext) {
    case 'rs':
      return { toolchain: 'rust', languageId: 'rust' };
    case 'ts':
    case 'tsx':
    case 'mts':
    case 'cts':
      return { toolchain: 'node', languageId: 'typescript' };
    case 'js':
    case 'jsx':
    case 'mjs':
    case 'cjs':
      return { toolchain: 'node', languageId: 'javascript' };
    default:
      return null;
  }
}

/** `rootUri` LSP dérivé du répertoire contenant `path` — `@codemirror/lsp-client` ne
 *  déduit jamais `rootUri` lui-même (vaut `null` si non fourni). Un LSP JS/TS remonte
 *  l'arborescence depuis la racine fournie pour trouver `package.json`/
 *  `node_modules` : un répertoire *sous* le projet suffit (pas besoin d'être pile
 *  dessus), donc le répertoire du fichier ouvert convient même profondément niché.
 *  Sans ce `rootUri`, le process retombe sur son cwd de spawn (racine du monorepo) —
 *  qui ne correspond au vrai projet que par coïncidence (rust ici, `Cargo.toml` y
 *  vit) ; pour un sous-projet comme `frontend/` (node/ts), ça ne trouve jamais le
 *  bon `node_modules` même après un `npm install` réel — trouvé en usage réel.
 *  Fichier à la racine (pas de `/`) → `file:///` (racine du workspace). */
export function dirRootUri(path: string): string {
  const idx = path.lastIndexOf('/');
  const dir = idx === -1 ? '' : path.slice(0, idx);
  return `file:///${dir}`;
}

/** Retourne l'extension CodeMirror pour `path`, déduite de son extension de
 *  fichier. `null`/pas d'extension reconnue → tableau vide (texte brut). */
export function languageExtensionForPath(path: string | null): Extension[] {
  if (!path) return [];
  const ext = path.split('.').pop()?.toLowerCase();
  const factory = ext ? byExtension[ext] : undefined;
  return factory ? [factory()] : [];
}
