import { describe, expect, it } from 'vitest';
import { StringStream } from '@codemirror/language';
import { handlebarsMode } from './langHandlebars';
import type { HbsState } from './langHandlebars';

/** Tokenise une ligne entière en paires `{ text, token }` (les blancs portent
 *  le tag `null`). Le même `state` est passé entre appels pour enchaîner les
 *  lignes (cas multi-lignes). `tabSize`/`indentUnit` sont exigés par le
 *  constructeur de `StringStream` mais sans effet ici : le mode ne consulte
 *  jamais `column()`. */
function tokenizeLine(
  line: string,
  state: HbsState,
): Array<{ text: string; token: string | null }> {
  const stream = new StringStream(line, 4, 2);
  const out: Array<{ text: string; token: string | null }> = [];
  while (!stream.eol()) {
    // Comme le conducteur de `StreamLanguage` (readToken) : la position de
    // départ du token courant est la position courante avant chaque appel,
    // sinon `stream.current()` s'accumule d'un token sur l'autre.
    stream.start = stream.pos;
    const tag = handlebarsMode.token(stream, state);
    out.push({ text: stream.current(), token: tag });
  }
  return out;
}

function newHbsState(): HbsState {
  return handlebarsMode.startState!(0);
}

describe('handlebarsMode', () => {
  it('startState : état initial, aucun commentaire ni balise ouverts', () => {
    expect(newHbsState()).toEqual({
      inHbsComment: false,
      inHtmlComment: false,
      inTag: false,
    });
  });

  it('ligne complète : balise, attribut, chaîne, moustache, fermeture', () => {
    const state = newHbsState();
    expect(tokenizeLine('<div class="app">{{ title }}</div>', state)).toEqual([
      { text: '<div', token: 'tagName' },
      { text: ' ', token: null },
      { text: 'class', token: 'attributeName' },
      { text: '=', token: 'operator' },
      { text: '"app"', token: 'string' },
      { text: '>', token: 'punctuation' },
      { text: '{{ title }}', token: 'variableName' },
      { text: '</div', token: 'tagName' },
      { text: '>', token: 'punctuation' },
    ]);
    expect(state).toEqual({ inHbsComment: false, inHtmlComment: false, inTag: false });
  });

  it('{{#if user}}Hi{{/if}} : sections en keyword, texte entre en null', () => {
    // Règle 14 : un caractère par token — `Hi` est donc deux tokens null.
    expect(tokenizeLine('{{#if user}}Hi{{/if}}', newHbsState())).toEqual([
      { text: '{{#if user}}', token: 'keyword' },
      { text: 'H', token: null },
      { text: 'i', token: null },
      { text: '{{/if}}', token: 'keyword' },
    ]);
  });

  it('{{^list}}…{{/list}} : section inversée et fermeture en keyword', () => {
    expect(tokenizeLine('{{^list}}…{{/list}}', newHbsState())).toEqual([
      { text: '{{^list}}', token: 'keyword' },
      { text: '…', token: null },
      { text: '{{/list}}', token: 'keyword' },
    ]);
  });

  it('{{else}} est un variableName (hors liste # ^ / — verrou de la simplification)', () => {
    expect(tokenizeLine('{{else}}', newHbsState())).toEqual([
      { text: '{{else}}', token: 'variableName' },
    ]);
  });

  it('{{! commentaire }} : un token comment, refermé par }}', () => {
    const state = newHbsState();
    expect(tokenizeLine('{{! commentaire }}', state)).toEqual([
      { text: '{{! commentaire }}', token: 'comment' },
    ]);
    expect(state.inHbsComment).toBe(false);
  });

  it('{{!-- dash --}} : comment refermé au premier }} (verrou règle 5)', () => {
    const state = newHbsState();
    expect(tokenizeLine('{{!-- dash --}}', state)).toEqual([
      { text: '{{!-- dash --}}', token: 'comment' },
    ]);
    expect(state.inHbsComment).toBe(false);
  });

  it('commentaire handlebars multi-ligne : comment des deux côtés, éteint', () => {
    const state = newHbsState();
    expect(tokenizeLine('{{! début', state)).toEqual([
      { text: '{{! début', token: 'comment' },
    ]);
    expect(state.inHbsComment).toBe(true);
    expect(tokenizeLine('fin }} <p>x</p>', state)).toEqual([
      { text: 'fin }}', token: 'comment' },
      { text: ' ', token: null },
      { text: '<p', token: 'tagName' },
      { text: '>', token: 'punctuation' },
      { text: 'x', token: null },
      { text: '</p', token: 'tagName' },
      { text: '>', token: 'punctuation' },
    ]);
    expect(state).toEqual({ inHbsComment: false, inHtmlComment: false, inTag: false });
  });

  it('commentaire HTML multi-ligne : comment des deux côtés, éteint à -->', () => {
    const state = newHbsState();
    expect(tokenizeLine('<!-- html comment', state)).toEqual([
      { text: '<!-- html comment', token: 'comment' },
    ]);
    expect(state.inHtmlComment).toBe(true);
    expect(tokenizeLine('multi -->', state)).toEqual([
      { text: 'multi -->', token: 'comment' },
    ]);
    expect(state.inHtmlComment).toBe(false);
  });

  it('attribut multi-ligne : inTag allumé traverse les lignes, > éteint', () => {
    const state = newHbsState();
    expect(tokenizeLine('<input type="text"', state)).toEqual([
      { text: '<input', token: 'tagName' },
      { text: ' ', token: null },
      { text: 'type', token: 'attributeName' },
      { text: '=', token: 'operator' },
      { text: '"text"', token: 'string' },
    ]);
    expect(state.inTag).toBe(true);
    expect(tokenizeLine('disabled>', state)).toEqual([
      { text: 'disabled', token: 'attributeName' },
      { text: '>', token: 'punctuation' },
    ]);
    expect(state.inTag).toBe(false);
  });

  it('{{{ raw }}} : un token variableName (préfixe long avant {{)', () => {
    expect(tokenizeLine('{{{ raw }}}', newHbsState())).toEqual([
      { text: '{{{ raw }}}', token: 'variableName' },
    ]);
  });

  it('texte simple : uniquement des null (règle 14, un caractère par token)', () => {
    const tokens = tokenizeLine('hello world', newHbsState());
    expect(tokens.every((t) => t.token === null)).toBe(true);
    expect(tokens.map((t) => t.text).join('')).toBe('hello world');
  });
});
