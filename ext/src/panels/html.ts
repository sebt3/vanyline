import { randomInt } from 'node:crypto';

/** Génère le HTML de la webview. Fonction pure (zéro import `vscode`, testable en node) :
 *  c'est le provider qui calcule `baseHref` et `cspSource` via `webview.asWebviewUri` /
 *  `webview.cspSource`. Repris de kydah-code `src/extension/panels/main.ts`. */
export function buildHtml(baseHref: string, cspSource: string, nonce: string): string {
  return `<!DOCTYPE html>
<html lang="fr">
  <head>
    <meta charset="UTF-8" />
    <meta http-equiv="Content-Security-Policy"
      content="default-src 'none';
               script-src 'nonce-${nonce}' ${cspSource};
               style-src ${cspSource} 'unsafe-inline';
               img-src ${cspSource} data:;
               font-src ${cspSource};" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <base href="${baseHref}/" />
    <title>vanyline</title>
    <style>html,body{height:100%;margin:0;padding:0}#app{height:100%;display:flex;flex-direction:column}</style>
    <link rel="stylesheet" href="${baseHref}/assets/index.css" />
  </head>
  <body>
    <div id="app"></div>
    <script type="module" nonce="${nonce}" src="${baseHref}/assets/index.js"></script>
  </body>
</html>`;
}

/** Nonce CSP : 32 caractères [A-Za-z0-9]. Aléa cryptographique (`node:crypto`) —
 *  un nonce CSP n'a de valeur que s'il est imprévisible, `Math.random()` ne l'est pas
 *  (la version kydah-code s'appuyait dessus). `randomInt` est sans biais modulo. */
export function generateNonce(): string {
  const possible =
    'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let text = '';
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(randomInt(possible.length));
  }
  return text;
}
