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
  ])('renvoie une extension non vide pour %s', (path) => {
    expect(languageExtensionForPath(path).length).toBeGreaterThan(0);
  });

  it('chemin sans extension reconnue → tableau vide', () => {
    expect(languageExtensionForPath('Dockerfile')).toEqual([]);
  });

  it('null → tableau vide', () => {
    expect(languageExtensionForPath(null)).toEqual([]);
  });

  it("l'extension est insensible à la casse", () => {
    expect(languageExtensionForPath('a.TS').length).toBeGreaterThan(0);
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

  it.each(['a.py', 'Dockerfile', null])('retourne null pour %s', (path) => {
    expect(lspToolchainForPath(path)).toBeNull();
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
