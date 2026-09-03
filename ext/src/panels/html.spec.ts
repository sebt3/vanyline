import { describe, expect, it } from 'vitest';
import { buildHtml, generateNonce } from './html';

describe('generateNonce', () => {
  it('fait 32 caractères', () => {
    expect(generateNonce()).toHaveLength(32);
  });

  it('n\'est composé que de [A-Za-z0-9]', () => {
    expect(generateNonce()).toMatch(/^[A-Za-z0-9]+$/);
  });

  it('deux appels diffèrent', () => {
    expect(generateNonce()).not.toBe(generateNonce());
  });
});

describe('buildHtml', () => {
  const html = buildHtml('https://base.example/dist/webview', 'vscode-webview://csp', 'NONCE123');

  it("embarque le CSP script-src 'nonce-…' + cspSource", () => {
    expect(html).toContain("script-src 'nonce-NONCE123' vscode-webview://csp");
  });

  it('<base href> pointé vers le dist webview', () => {
    expect(html).toContain('<base href="https://base.example/dist/webview/"');
  });

  it('référence index.js avec le nonce', () => {
    expect(html).toContain('src="https://base.example/dist/webview/assets/index.js"');
    expect(html).toContain('nonce="NONCE123"');
  });

  it('référence index.css', () => {
    expect(html).toContain('href="https://base.example/dist/webview/assets/index.css"');
  });

  it("ne contient pas d'unsafe-eval", () => {
    expect(html).not.toContain('unsafe-eval');
  });

  // Vue : balise lue par le bundle webview (router.resolveView). Le défaut 'chat'
  // garde les tests 3-args ci-dessus inchangés ; 'config' explicite pour le panel.
  it("meta vanyline-view = chat par défaut (3 args — comportement F3)", () => {
    expect(html).toContain('<meta name="vanyline-view" content="chat"');
    expect(buildHtml('https://base.example/dist/webview', 'vscode-webview://csp', 'NONCE123')).toContain(
      '<meta name="vanyline-view" content="chat"',
    );
  });

  it("meta vanyline-view = config en 4ᵉ argument explicite", () => {
    const configHtml = buildHtml(
      'https://base.example/dist/webview',
      'vscode-webview://csp',
      'NONCE123',
      'config',
    );
    expect(configHtml).toContain('<meta name="vanyline-view" content="config"');
  });
});
