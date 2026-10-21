// Test de garde (contrat : ./shell.sdd). Pas de rendu pour les déclarations CSS :
// on lit les sources. Les regex tolèrent les espaces.
/// <reference types="node" />
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SHELL_DIR = dirname(fileURLToPath(import.meta.url));
const FRONTEND_SRC = resolve(SHELL_DIR, '../..');
const THEME_CSS = join(FRONTEND_SRC, 'theme.css');
const APP_NAV = join(SHELL_DIR, 'AppNav.vue');
const ASSISTANT_PANEL = join(SHELL_DIR, 'AssistantPanel.vue');

const TOKENS: Record<string, string> = {
  '--bg': '#0f1012',
  '--surface': '#16181b',
  '--surface-2': '#1d2024',
  '--border': '#2a2e33',
  '--text': '#e8eaed',
  '--muted': '#9aa1ab',
  '--accent': '#8b9bff',
  '--accent-bg': '#1f2547',
  '--accent-text': '#c4ccff',
  '--ok': '#4ade80',
  '--warn': '#fbbf24',
  '--err': '#f87171',
};

/** Corps de la première règle `selector { … }` (sélecteur exact). */
function ruleBody(source: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`(?:^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm');
  const match = re.exec(source);
  if (!match) throw new Error(`règle introuvable : ${selector}`);
  return match[1] ?? '';
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  const [r = 0, g = 0, b = 0] = channels;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const HEX = /#[0-9a-fA-F]{3,8}\b/;

describe('shell — jetons_du_theme', () => {
  const css = readFileSync(THEME_CSS, 'utf8');
  const root = ruleBody(css, ':root');

  it('définit les douze jetons de couleur sur :root avec les valeurs de shell.sdd', () => {
    for (const [name, value] of Object.entries(TOKENS)) {
      const re = new RegExp(`${name}\\s*:\\s*${value}\\s*;`, 'i');
      expect(root, `jeton ${name}`).toMatch(re);
    }
  });

  it('définit les polices et la hauteur du rail/barre', () => {
    expect(root).toMatch(/--font-sans\s*:[^;]*Inter[^;]*system-ui/);
    expect(root).toMatch(/--font-mono\s*:[^;]*JetBrains Mono[^;]*monospace/);
    expect(root).toMatch(/--nav-h\s*:/);
  });

  it("ni AppNav.vue ni AssistantPanel.vue ne contiennent de couleur hexadécimale", () => {
    expect(HEX.test(readFileSync(APP_NAV, 'utf8'))).toBe(false);
    expect(HEX.test(readFileSync(ASSISTANT_PANEL, 'utf8'))).toBe(false);
  });
});

describe('shell — contraste (Must)', () => {
  it('--text et --muted atteignent 4,5:1 sur --bg, --surface et --surface-2', () => {
    for (const fg of ['--text', '--muted']) {
      for (const bg of ['--bg', '--surface', '--surface-2']) {
        expect(contrast(TOKENS[fg] ?? '', TOKENS[bg] ?? ''), `${fg} sur ${bg}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe('shell — nav_barre_sous_md', () => {
  const source = readFileSync(APP_NAV, 'utf8');

  it('une règle @media (max-width: 767px) bascule la nav en barre inférieure, entrée à var(--ctl-h)', () => {
    const block = /@media\s*\(\s*max-width\s*:\s*767px\s*\)\s*\{([\s\S]*)/.exec(source);
    expect(block).not.toBeNull();
    expect(block?.[1] ?? '').toMatch(/min-height\s*:\s*var\(--ctl-h\)/);
    expect(block?.[1] ?? '').toMatch(/bottom|flex-direction\s*:\s*row/);
  });
});

describe('shell — panneau_feuille_sous_md', () => {
  const source = readFileSync(ASSISTANT_PANEL, 'utf8');

  it('sous 768px : position fixed plein écran ; à partir de md : colonne de 340px', () => {
    const small = /@media\s*\(\s*max-width\s*:\s*767px\s*\)\s*\{([\s\S]*?)\n\}/.exec(source);
    expect(small?.[1] ?? '').toMatch(/position\s*:\s*fixed/);
    expect(small?.[1] ?? '').toMatch(/inset\s*:\s*0/);
    const wide = /@media\s*\(\s*min-width\s*:\s*768px\s*\)\s*\{([\s\S]*?)\n\}/.exec(source);
    expect(wide?.[1] ?? '').toMatch(/width\s*:\s*340px/);
  });
});
