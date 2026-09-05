import type { Component } from 'vue';
import {
  Cpu, Files, Connection, DataLine, Notebook, SetUp, Box, DataBoard,
  Folder, Document, MagicStick, Goods, Reading, Film,
} from '@element-plus/icons-vue';
import { dockerfileName } from './dockerfileName';

/** Icône dossier (dossier fermé). */
export const folderIcon = Folder;
/** Icône fichier générique (fallback). */
export const genericFileIcon = Document;

/** Mapping extension → icône — mêmes clés que `byExtension` (editorLanguage.ts).
 *  Premier set curé à la main, ajustable : ts/tsx → Cpu, js/jsx/mjs/cjs →
 *  Files, rs → Connection, json → DataLine, md/markdown → Notebook,
 *  yaml/yml → SetUp, toml → Box, py → DataBoard, vue → MagicStick,
 *  rhai → Reading, hbs/handlebars → Film. Dockerfile/Containerfile → Goods
 *  par test de nom de base (`dockerfileName`, dans `iconForPath` avant le
 *  découpage d'extension — même ordering que `languageExtensionForPath`). */
const byExtension: Record<string, Component> = {
  ts: Cpu,
  tsx: Cpu,
  js: Files,
  jsx: Files,
  mjs: Files,
  cjs: Files,
  rs: Connection,
  json: DataLine,
  md: Notebook,
  markdown: Notebook,
  yaml: SetUp,
  yml: SetUp,
  toml: Box,
  py: DataBoard,
  vue: MagicStick,
  rhai: Reading,
  hbs: Film,
  handlebars: Film,
};

/** Icône pour un chemin relatif : déduite de son extension (lowercase) ;
 *  chemin sans extension connue ou null → `genericFileIcon`. */
export function iconForPath(path: string | null): Component {
  if (!path) return genericFileIcon;
  if (dockerfileName(path)) return Goods;
  const ext = path.split('.').pop()?.toLowerCase();
  return ext ? byExtension[ext] ?? genericFileIcon : genericFileIcon;
}
