import { describe, expect, it } from 'vitest';
import { dirRootUri, languageExtensionForPath, lspToolchainForPath } from './editorLanguage';

describe('languageExtensionForPath', () => {
  it.each([
    ['a.ts', 'ts'],
    ['a.tsx', 'tsx'],
    ['a.js', 'js'],
    ['a.jsx', 'jsx'],
    ['a.rs', 'rs'],
    ['a.json', 'json'],
    ['README.md', 'md'],
    ['a.toml', 'toml'],
    ['a.yaml', 'yaml'],
    ['a.yml', 'yml'],
    ['a.py', 'py'],
    ['a.vue', 'vue'],
    ['a.rhai', 'rhai'],
    ['a.hbs', 'hbs'],
    ['a.handlebars', 'handlebars'],
  ])('renvoie une extension non vide pour %s', (path) => {
    expect(languageExtensionForPath(path).length).toBeGreaterThan(0);
  });

  it('chemin sans extension reconnue → tableau vide', () => {
    expect(languageExtensionForPath('Makefile')).toEqual([]);
  });

  it('nom de base Dockerfile → mode dockerfile (tableau de longueur 1)', () => {
    expect(languageExtensionForPath('Dockerfile')).toHaveLength(1);
    expect(languageExtensionForPath('deploy/Dockerfile.dev')).toHaveLength(1);
    expect(languageExtensionForPath('app.dockerfile')).toHaveLength(1);
  });

  it('Dockerfile reconnu alors que le chemin ne contient aucun point', () => {
    // `path.split('.').pop()` seul ne suffirait pas : 'Dockerfile' n'a pas
    // d'extension, c'est un nom de base.
    expect('Dockerfile').not.toContain('.');
    expect(languageExtensionForPath('Dockerfile')).not.toEqual([]);
  });

  it('null → tableau vide', () => {
    expect(languageExtensionForPath(null)).toEqual([]);
  });

  it('.vue avec chemin à points multiples', () => {
    // L'extension retenue est bien `vue` (dernier segment), pas `bar`.
    expect(languageExtensionForPath('src/components/Foo.bar.vue')).toHaveLength(1);
  });

  it("l'extension est insensible à la casse", () => {
    expect(languageExtensionForPath('a.TS').length).toBeGreaterThan(0);
    expect(languageExtensionForPath('App.VUE')).toHaveLength(1);
  });
});

describe('lspToolchainForPath', () => {
  it.each([
    ['src/main.rs', { toolchain: 'rust', languageId: 'rust' }],
    ['a.ts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.tsx', { toolchain: 'node', languageId: 'typescript' }],
    ['a.mts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.cts', { toolchain: 'node', languageId: 'typescript' }],
    ['a.js', { toolchain: 'node', languageId: 'javascript' }],
    ['a.jsx', { toolchain: 'node', languageId: 'javascript' }],
    ['a.mjs', { toolchain: 'node', languageId: 'javascript' }],
    ['a.cjs', { toolchain: 'node', languageId: 'javascript' }],
  ])('%s → %s', (path, expected) => {
    expect(lspToolchainForPath(path)).toEqual(expected);
  });

  it.each(['a.py', 'a.rhai', 'a.hbs', 'Dockerfile', null])('retourne null pour %s', (path) => {
    expect(lspToolchainForPath(path)).toBeNull();
  });

  it('.vue reste sans LSP (verrou de périmètre : coloration seule)', () => {
    expect(lspToolchainForPath('App.vue')).toBeNull();
  });

  it('l\'extension est insensible à la casse', () => {
    expect(lspToolchainForPath('A.RS')).toEqual({ toolchain: 'rust', languageId: 'rust' });
    expect(lspToolchainForPath('A.TS')).toEqual({ toolchain: 'node', languageId: 'typescript' });
    expect(lspToolchainForPath('A.JS')).toEqual({ toolchain: 'node', languageId: 'javascript' });
  });
});

describe('dirRootUri', () => {
  it('fichier niché : répertoire contenant le fichier', () => {
    expect(dirRootUri('frontend/src/components/panels/Editor.vue'))
      .toBe('file:///frontend/src/components/panels');
  });

  it('fichier directement sous un répertoire : ce répertoire', () => {
    expect(dirRootUri('frontend/App.vue')).toBe('file:///frontend');
  });

  it('fichier à la racine (pas de /) : racine du workspace', () => {
    expect(dirRootUri('Cargo.toml')).toBe('file:///');
  });
});
