/// <reference types="node" />
// Test de garde (contrat : ./layout.sdd). Pas de rendu : on lit les sources
// et on vérifie les déclarations CSS. Les regex tolèrent les espaces ; si une
// règle est réécrite autrement sans changer le comportement, adapter la regex
// plutôt que de la contourner.
import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const FRONTEND_SRC = dirname(fileURLToPath(import.meta.url));
const UI_SRC = resolve(FRONTEND_SRC, '../../packages/ui/src');
const STYLE_CSS = resolve(FRONTEND_SRC, 'style.css');

function collectStyleSources(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      out.push(...collectStyleSources(path));
    } else if (/\.(vue|css)$/.test(name)) {
      out.push(path);
    }
  }
  return out;
}

/** Corps de la première règle `selector { … }` (sélecteur exact, en début de ligne ou après `}`). */
function ruleBody(source: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`(?:^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm');
  const match = re.exec(source);
  if (!match) throw new Error(`règle introuvable : ${selector}`);
  return match[1] ?? '';
}

const HARD_44 = /(?<![-\w])(?:min-)?height\s*:\s*44px\b/;

describe('layout — aucune_hauteur_tactile_en_dur', () => {
  it('aucun min-height / height 44px en dur hors de style.css', () => {
    const files = [...collectStyleSources(FRONTEND_SRC), ...collectStyleSources(UI_SRC)].filter(
      (f) => resolve(f) !== STYLE_CSS,
    );
    expect(files.length).toBeGreaterThan(0);
    const offenders = files.filter((f) => HARD_44.test(readFileSync(f, 'utf8')));
    expect(offenders).toEqual([]);
  });

  it('style.css définit --ctl-h à 28px par défaut et 44px sous pointer: coarse / max-width 767px', () => {
    const css = readFileSync(STYLE_CSS, 'utf8');
    expect(css).toMatch(/:root\s*\{[^}]*--ctl-h\s*:\s*28px\s*;?[^}]*\}/);
    expect(css).toMatch(
      /@media\s*\(\s*pointer\s*:\s*coarse\s*\)\s*,\s*\(\s*max-width\s*:\s*767px\s*\)\s*\{[\s\S]*?--ctl-h\s*:\s*44px/,
    );
  });
});

describe('layout — la_page_des_reglages_ne_defile_pas', () => {
  const source = readFileSync(join(FRONTEND_SRC, 'components/SettingsView.vue'), 'utf8');

  it('.settings-view : height 100%, overflow hidden, min-height 0', () => {
    const body = ruleBody(source, '.settings-view');
    expect(body).toMatch(/(?<![-\w])height\s*:\s*100%/);
    expect(body).toMatch(/(?<![-\w])overflow\s*:\s*hidden/);
    expect(body).toMatch(/(?<![-\w])min-height\s*:\s*0\b/);
  });

  it('.settings-main : défilement interne (overflow auto) et min-height 0', () => {
    const body = ruleBody(source, '.settings-main');
    expect(body).toMatch(/(?<![-\w])overflow(?:-y)?\s*:\s*auto/);
    expect(body).toMatch(/(?<![-\w])min-height\s*:\s*0\b/);
  });

  it('.settings-assistant : min-height 0', () => {
    const body = ruleBody(source, '.settings-assistant');
    expect(body).toMatch(/(?<![-\w])min-height\s*:\s*0\b/);
  });
});

describe('layout — le_chat_contextuel_occupe_le_reste_de_la_colonne', () => {
  const source = readFileSync(join(FRONTEND_SRC, 'components/ContextChat.vue'), 'utf8');

  it('.context-chat : flex 1 et min-height 0, sans height 100%', () => {
    const body = ruleBody(source, '.context-chat');
    expect(body).toMatch(/(?<![-\w])flex\s*:\s*1\b/);
    expect(body).toMatch(/(?<![-\w])min-height\s*:\s*0\b/);
    expect(body).not.toMatch(/(?<![-\w])height\s*:\s*100%/);
  });
});
