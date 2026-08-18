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

/** Retourne l'extension CodeMirror pour `path`, déduite de son extension de
 *  fichier. `null`/pas d'extension reconnue → tableau vide (texte brut). */
export function languageExtensionForPath(path: string | null): Extension[] {
  if (!path) return [];
  const ext = path.split('.').pop()?.toLowerCase();
  const factory = ext ? byExtension[ext] : undefined;
  return factory ? [factory()] : [];
}
