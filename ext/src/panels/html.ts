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

/** Nonce CSP : 32 caractères [A-Za-z0-9] (repris de kydah-code generateNonce). */
export function generateNonce(): string {
  let text = '';
  const possible =
    'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}
