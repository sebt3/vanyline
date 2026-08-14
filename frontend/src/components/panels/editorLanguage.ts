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

/** Retourne l'extension CodeMirror pour `path`, déduite de son extension de
 *  fichier. `null`/pas d'extension reconnue → tableau vide (texte brut). */
export function languageExtensionForPath(path: string | null): Extension[] {
  if (!path) return [];
  const ext = path.split('.').pop()?.toLowerCase();
  const factory = ext ? byExtension[ext] : undefined;
  return factory ? [factory()] : [];
}
