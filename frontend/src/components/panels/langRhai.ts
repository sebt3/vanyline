import type { StreamParser, StringStream } from '@codemirror/language';

/** État du tokenizer rhai : traversal des constructions multi-lignes. */
export interface RhaiState {
  inBlockComment: boolean; // à l'intérieur d'un /* ... */ (non imbriqué)
  inRawString: boolean;    // à l'intérieur d'une chaîne backtick multi-ligne
}

/** Liste fermée des mots-clés (design editor-syntax-highlighting, risque 2) :
 *  rien de plus — un mot hors liste est un `variableName`, même préfixé. */
const KEYWORDS = new Set([
  'let', 'fn', 'if', 'else', 'while', 'loop', 'for', 'in', 'return', 'true', 'false',
]);

/** Consomme la fin d'un commentaire bloc entamé : jusqu'au délimiteur fermant
 *  (étoile puis barre oblique, consommé, `inBlockComment` refermé) ou fin de
 *  ligne (état laissé ouvert). */
function readBlockCommentTail(stream: StringStream, state: RhaiState): void {
  if (stream.skipTo('*/')) {
    stream.next(); // `*`
    stream.next(); // `/`
    state.inBlockComment = false;
  } else {
    stream.skipToEnd();
  }
}

/** Consomme une chaîne `"` entamée : jusqu'au `"` fermant (`\` échappe le
 *  caractère suivant) ou fin de ligne. Pas d'état multi-ligne : une chaîne `"`
 *  non fermée en fin de ligne ne déborde pas sur la ligne suivante. */
function readQuotedTail(stream: StringStream): void {
  while (!stream.eol()) {
    const ch = stream.next();
    if (ch === '\\') stream.next();
    else if (ch === '"') return;
  }
}

/** Consomme une chaîne backtick entamée : jusqu'au backtick fermant ou fin de
 *  ligne — non fermée → `inRawString` (la suite continue sur les lignes
 *  suivantes, en `string`). */
function readRawStringTail(stream: StringStream, state: RhaiState): void {
  if (stream.skipTo('`')) {
    stream.next();
    state.inRawString = false;
  } else {
    stream.skipToEnd();
    state.inRawString = true;
  }
}

/** Mode StreamLanguage maison pour rhai — périmètre volontairement limité
 *  (design editor-syntax-highlighting, risque 2) : mots-clés, commentaires,
 *  chaînes " et backtick, nombres. */
export const rhaiMode: StreamParser<RhaiState> = {
  startState(): RhaiState {
    return { inBlockComment: false, inRawString: false };
  },

  token(stream, state) {
    // États multi-lignes d'abord : les blancs et le reste de la ligne
    // appartiennent alors au commentaire / à la chaîne en cours.
    if (state.inBlockComment) {
      readBlockCommentTail(stream, state);
      return 'comment';
    }
    if (state.inRawString) {
      readRawStringTail(stream, state);
      return 'string';
    }

    if (stream.eatSpace()) return null;

    // Commentaires : // jusqu'en fin de ligne ; /* non imbriqué, état
    // inBlockComment tant que le */ n'est pas trouvé.
    if (stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }
    if (stream.match('/*')) {
      state.inBlockComment = true;
      readBlockCommentTail(stream, state);
      return 'comment';
    }

    // Chaînes : " avec échappement \ (une ligne) ; backtick multi-ligne.
    if (stream.match('"')) {
      readQuotedTail(stream);
      return 'string';
    }
    if (stream.match('`')) {
      readRawStringTail(stream, state);
      return 'string';
    }

    // Nombres : premier chiffre puis la classe large `[0-9a-fA-FxX._]` —
    // couvre 42, 3.14, 0xff (coloration « suffisante », pas un parseur).
    if (stream.eat(/[0-9]/)) {
      stream.eatWhile(/[0-9a-fA-FxX._]/);
      return 'number';
    }

    // Mots : mots-clés (liste fermée) sinon identifiants.
    if (stream.eat(/[A-Za-z_]/)) {
      stream.eatWhile(/[A-Za-z0-9_]/);
      return KEYWORDS.has(stream.current()) ? 'keyword' : 'variableName';
    }

    // Ponctuation : un caractère des délimiteurs reconnus.
    if (stream.eat(/[()[\]{},;.]/)) return 'punctuation';

    // Tout autre caractère non-blanc : opérateur, un caractère.
    stream.next();
    return 'operator';
  },
};
