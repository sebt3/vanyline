import { describe, expect, it } from 'vitest';
import { StringStream } from '@codemirror/language';
import { rhaiMode } from './langRhai';
import type { RhaiState } from './langRhai';

/** Tokenise une ligne entière en paires `{ text, token }` (les blancs portent
 *  le tag `null`). Le même `state` est passé entre appels pour enchaîner les
 *  lignes (cas multi-lignes). `tabSize`/`indentUnit` sont exigés par le
 *  constructeur de `StringStream` mais sans effet ici : le mode ne consulte
 *  jamais `column()`. */
function tokenizeLine(
  line: string,
  state: RhaiState,
): Array<{ text: string; token: string | null }> {
  const stream = new StringStream(line, 4, 2);
  const out: Array<{ text: string; token: string | null }> = [];
  while (!stream.eol()) {
    // Comme le conducteur de `StreamLanguage` (readToken) : la position de
    // départ du token courant est la position courante avant chaque appel,
    // sinon `stream.current()` s'accumule d'un token sur l'autre.
    stream.start = stream.pos;
    const tag = rhaiMode.token(stream, state);
    out.push({ text: stream.current(), token: tag });
  }
  return out;
}

function newRhaiState(): RhaiState {
  return rhaiMode.startState!(0);
}

describe('rhaiMode', () => {
  it('startState : état initial, ni commentaire ni raw string ouverts', () => {
    expect(newRhaiState()).toEqual({ inBlockComment: false, inRawString: false });
  });

  it('ligne complète : mots-clés, identifiants, ponctuation, opérateur', () => {
    const tokens = tokenizeLine(
      'let x = fn() { if a { return true } else { return false } }',
      newRhaiState(),
    );
    const tagByWord = new Map(tokens.map((t) => [t.text, t.token]));
    for (const kw of ['let', 'fn', 'if', 'return', 'true', 'else']) {
      expect(tagByWord.get(kw)).toBe('keyword');
    }
    for (const ident of ['x', 'a']) {
      expect(tagByWord.get(ident)).toBe('variableName');
    }
    for (const punct of ['{', '}', '(', ')']) {
      expect(tagByWord.get(punct)).toBe('punctuation');
    }
    expect(tagByWord.get('=')).toBe('operator');
  });

  it('chaîne " avec guillemets échappés : \\ protège le caractère suivant', () => {
    // Contrat (tableau) : la chaîne court jusqu'au `"` fermant, `\` échappant
    // le caractère suivant — ici le second `\"` est protégé, la chaîne ne
    // ferme donc qu'au dernier `" : le token couvre toute la chaîne source.
    const tokens = tokenizeLine('let s = "bon \\"jour\\" x"', newRhaiState());
    const strings = tokens.filter((t) => t.token === 'string');
    expect(strings).toHaveLength(1);
    expect(strings[0]?.text).toBe('"bon \\"jour\\" x"');
  });

  it('x // commentaire : le dernier token est le commentaire', () => {
    const tokens = tokenizeLine('x // commentaire', newRhaiState());
    expect(tokens[tokens.length - 1]).toEqual({ text: '// commentaire', token: 'comment' });
  });

  it('// seul : tout le reste de la ligne en comment', () => {
    expect(tokenizeLine('// todo: plus tard', newRhaiState())).toEqual([
      { text: '// todo: plus tard', token: 'comment' },
    ]);
  });

  it('commentaire bloc multi-ligne : comment des deux côtés, refermé sur */', () => {
    const state = newRhaiState();
    expect(tokenizeLine('a /* début', state)).toEqual([
      { text: 'a', token: 'variableName' },
      { text: ' ', token: null },
      { text: '/* début', token: 'comment' },
    ]);
    expect(state.inBlockComment).toBe(true);
    expect(tokenizeLine('fin */ b', state)).toEqual([
      { text: 'fin */', token: 'comment' },
      { text: ' ', token: null },
      { text: 'b', token: 'variableName' },
    ]);
    expect(state.inBlockComment).toBe(false);
  });

  it('chaîne backtick multi-ligne : inRawString allumé puis éteint', () => {
    const state = newRhaiState();
    const first = tokenizeLine('let r = `ligne1', state);
    expect(first).toEqual([
      { text: 'let', token: 'keyword' },
      { text: ' ', token: null },
      { text: 'r', token: 'variableName' },
      { text: ' ', token: null },
      { text: '=', token: 'operator' },
      { text: ' ', token: null },
      { text: '`ligne1', token: 'string' },
    ]);
    expect(state.inRawString).toBe(true);
    expect(tokenizeLine('ligne2`', state)).toEqual([{ text: 'ligne2`', token: 'string' }]);
    expect(state.inRawString).toBe(false);
  });

  it.each(['42', '3.14', '0xff'])('nombre %s : un seul token number', (src) => {
    expect(tokenizeLine(src, newRhaiState())).toEqual([{ text: src, token: 'number' }]);
  });

  it.each(['loop', 'for', 'while', 'in'])('%s est un mot-clé', (kw) => {
    expect(tokenizeLine(kw, newRhaiState())).toEqual([{ text: kw, token: 'keyword' }]);
  });

  it('lets (préfixe de mot-clé mais hors liste) est un variableName', () => {
    expect(tokenizeLine('lets', newRhaiState())).toEqual([
      { text: 'lets', token: 'variableName' },
    ]);
  });
});
