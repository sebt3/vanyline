import { describe, expect, it } from 'vitest';
import { languageExtensionForPath } from './editorLanguage';

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
