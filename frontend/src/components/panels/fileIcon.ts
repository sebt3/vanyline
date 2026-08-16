import type { Component } from 'vue';
import {
  Cpu, Files, Connection, DataLine, Notebook, SetUp, Box, DataBoard,
  Folder, Document,
} from '@element-plus/icons-vue';

/** Icône dossier (dossier fermé). */
export const folderIcon = Folder;
/** Icône fichier générique (fallback). */
export const genericFileIcon = Document;

/** Mapping extension → icône — mêmes clés que `byExtension` (editorLanguage.ts).
 *  Premier set curé à la main, ajustable : ts/tsx → Cpu, js/jsx/mjs/cjs →
 *  Files, rs → Connection, json → DataLine, md/markdown → Notebook,
 *  yaml/yml → SetUp, toml → Box, py → DataBoard. */
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
};

/** Icône pour un chemin relatif : déduite de son extension (lowercase) ;
 *  chemin sans extension connue ou null → `genericFileIcon`. */
export function iconForPath(path: string | null): Component {
  if (!path) return genericFileIcon;
  const ext = path.split('.').pop()?.toLowerCase();
  return ext ? byExtension[ext] ?? genericFileIcon : genericFileIcon;
}
