import { describe, it, expect } from 'vitest';
import { reconstructBase } from './diffPatch';

describe('reconstructBase', () => {
  it('reconstruit la base depuis un hunk ligne modifiée', () => {
    const working = 'ligne1\nligne2 MODIFIEE\nligne3\n';
    const patch = [
      'diff --git a/x.txt b/x.txt',
      'index 111..222 100644',
      '--- a/x.txt',
      '+++ b/x.txt',
      '@@ -1,3 +1,3 @@',
      ' ligne1',
      '-ligne2',
      '+ligne2 MODIFIEE',
      ' ligne3',
    ].join('\n');
    const result = reconstructBase(working, patch);
    expect(result).toBe('ligne1\nligne2\nligne3\n');
  });

  it('reconstruit la base depuis un hunk avec ajout', () => {
    const working = 'a\nb\nc\n';
    const patch = [
      'diff --git a/x b/x',
      'index .. 100644',
      '@@ -1,2 +1,3 @@',
      ' a',
      '+b',
      ' c',
    ].join('\n');
    expect(reconstructBase(working, patch)).toBe('a\nc\n');
  });

  it('reconstruit la base depuis un hunk avec suppression', () => {
    const working = 'a\nc\n';
    const patch = [
      'diff --git a/x b/x',
      'index .. 100644',
      '@@ -1,3 +1,2 @@',
      ' a',
      '-b',
      ' c',
    ].join('\n');
    expect(reconstructBase(working, patch)).toBe('a\nb\nc\n');
  });

  it('patch vide → base identique à working', () => {
    expect(reconstructBase('a\nb\n', '')).toBe('a\nb\n');
  });

  it('patch multi-hunks appliqué de la fin vers le début', () => {
    // Working tree avec deux modifications (ligne 1 et ligne 10)
    const working =
      'CHANGED_01\n02\n03\n04\n05\n06\n07\n08\n09\nCHANGED_10\n';
    const patch = [
      'diff --git a/x b/x',
      '@@ -1,3 +1,3 @@',
      '-CHANGED_01',
      '+01',
      ' 02',
      ' 03',
      '@@ -9,2 +9,2 @@',
      ' 09',
      '-CHANGED_10',
      '+10',
    ].join('\n');
    const result = reconstructBase(working, patch);
    expect(result).toBe('01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n');
  });

  it('patch sans hunk (en-têtes seuls) → working inchangé', () => {
    const patch = 'diff --git a/x b/x\nindex .. 100644\n--- a/x\n+++ b/x\n';
    expect(reconstructBase('a\nb\n', patch)).toBe('a\nb\n');
  });
});