export type WebviewKind = 'chat' | 'config';

/** Valeur de <meta name="vanyline-view"> → racine à monter.
 *  Faute sécuritaire vers 'chat' pour toute valeur absente ou inconnue
 *  (une webview sans meta reste le comportement F3). */
export function resolveView(meta: string | null | undefined): WebviewKind {
  return meta === 'config' ? 'config' : 'chat';
}
