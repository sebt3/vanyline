import type { StreamParser, StringStream } from '@codemirror/language';

/** État du tokenizer handlebars : commentaires multi-lignes et intérieur de
 *  balise HTML (les attributs y sont des attributeName). */
export interface HbsState {
  inHbsComment: boolean;   // à l'intérieur de {{! ... }} ou {{!-- ... --}}
  inHtmlComment: boolean;  // à l'intérieur de <!-- ... -->
  inTag: boolean;          // entre `<nom` et le `>` fermant
}

/** Consomme la fin d'un commentaire handlebars entamé : jusqu'au `}}` suivant
 *  (deux accolades consommées, `inHbsComment` refermé) ou fin de ligne (état
 *  laissé ouvert). La forme `{{!--` se ferme déjà au premier `}}` —
 *  simplification actée du contrat (pas de recherche de `--}}`). */
function readHbsCommentTail(stream: StringStream, state: HbsState): void {
  if (stream.skipTo('}}')) {
    stream.next(); // `}`
    stream.next(); // `}`
    state.inHbsComment = false;
  } else {
    stream.skipToEnd();
  }
}

/** Consomme la fin d'un commentaire HTML entamé : jusqu'au `-->` suivant (trois
 *  caractères consommés, `inHtmlComment` refermé) ou fin de ligne (état laissé
 *  ouvert). */
function readHtmlCommentTail(stream: StringStream, state: HbsState): void {
  if (stream.skipTo('-->')) {
    stream.next(); // `-`
    stream.next(); // `-`
    stream.next(); // `>`
    state.inHtmlComment = false;
  } else {
    stream.skipToEnd();
  }
}

/** Consomme la fin d'une moustache : jusqu'au `}}` suivant (consommé) ou fin de
 *  ligne. Pas d'état : une moustache non fermée en fin de ligne ne déborde pas
 *  sur la ligne suivante. */
function readMustacheTail(stream: StringStream): void {
  if (stream.skipTo('}}')) {
    stream.next(); // `}`
    stream.next(); // `}`
  } else {
    stream.skipToEnd();
  }
}

/** Consomme la fin d'une moustache triple `{{{ ... }}}` : jusqu'au `}}}`
 *  suivant (trois accolades consommées) ou fin de ligne. */
function readTripleMustacheTail(stream: StringStream): void {
  if (stream.skipTo('}}}')) {
    stream.next(); // `}`
    stream.next(); // `}`
    stream.next(); // `}`
  } else {
    stream.skipToEnd();
  }
}

/** Consomme une chaîne entamée (`"` ou `'`) : jusqu'au guillemet fermant
 *  homologue ou fin de ligne. Pas d'échappement (HTML-lite), pas d'état : une
 *  chaîne non fermée en fin de ligne ne déborde pas sur la ligne suivante. */
function readQuotedTail(stream: StringStream, quote: string): void {
  while (!stream.eol()) {
    const ch = stream.next();
    if (ch === quote) return;
  }
}

/** Mode StreamLanguage maison pour handlebars — HTML-lite + surcouche
 *  moustaches, sans AST (design editor-syntax-highlighting). L'ordre des
 *  règles du token() est contractuel : préfixes longs d'abord (`{{!--`/`{{!`
 *  avant `{{#`/`{{^`/`{{/`, `{{#`/`{{^`/`{{/` avant `{{{`, `{{{` avant `{{`,
 *  `<!--` avant `<`). */
export const handlebarsMode: StreamParser<HbsState> = {
  startState(): HbsState {
    return { inHbsComment: false, inHtmlComment: false, inTag: false };
  },

  token(stream, state) {
    // États multi-lignes d'abord : le reste de la ligne appartient alors au
    // commentaire en cours (règles 1 et 2).
    if (state.inHbsComment) {
      readHbsCommentTail(stream, state);
      return 'comment';
    }
    if (state.inHtmlComment) {
      readHtmlCommentTail(stream, state);
      return 'comment';
    }

    // Blancs (règle 3).
    if (stream.eatSpace()) return null;

    // Commentaires (règles 4 et 5) : `<!--` HTML, puis moustaches de comment
    // handlebars — `{{!--` testé avant `{{!`, les deux s'allument en
    // inHbsComment et se ferment au premier `}}` (simplification actée).
    if (stream.match('<!--')) {
      state.inHtmlComment = true;
      readHtmlCommentTail(stream, state);
      return 'comment';
    }
    if (stream.match('{{!--') || stream.match('{{!')) {
      state.inHbsComment = true;
      readHbsCommentTail(stream, state);
      return 'comment';
    }

    // Sections (règle 6) : `{{# ...}}`, `{{^ ...}}` (inversée) et `{{/ ...}}`
    // (fermeture) — la moustache entière est un keyword.
    if (stream.match('{{#') || stream.match('{{^') || stream.match('{{/')) {
      readMustacheTail(stream);
      return 'keyword';
    }

    // Sortie brute `{{{ ... }}}` (règle 7) avant la moustache simple : sinon
    // `{{` avalerait le préfixe et la moustache se fermerait trop tôt.
    if (stream.match('{{{')) {
      readTripleMustacheTail(stream);
      return 'variableName';
    }

    // Moustache simple `{{ ... }}` (règle 8).
    if (stream.match('{{')) {
      readMustacheTail(stream);
      return 'variableName';
    }

    // Ouverture/fermeture de balise `<nom`, `</nom` (règle 9) : le nom jusqu'au
    // délimiteur, puis l'intérieur de la balise est attendu (inTag).
    if (stream.match(/<\/?[A-Za-z][A-Za-z0-9-]*/)) {
      state.inTag = true;
      return 'tagName';
    }

    // Intérieur de balise (règles 10 à 12) : `>` referme, mots = attributs,
    // `=` = opérateur.
    if (state.inTag) {
      if (stream.eat('>')) {
        state.inTag = false;
        return 'punctuation';
      }
      if (stream.match(/[A-Za-z_][A-Za-z0-9_-]*/)) return 'attributeName';
      if (stream.eat('=')) return 'operator';
    }

    // Chaînes (règle 13), dedans ou dehors une balise.
    const quote = stream.peek();
    if (quote === '"' || quote === "'") {
      stream.next(); // guillemet ouvrant
      readQuotedTail(stream, quote);
      return 'string';
    }

    // Tout le reste (règle 14) : un caractère, sans coloration — texte brut,
    // `<` non suivi d'une lettre, mot hors balise (pas attributeName : seul
    // l'intérieur d'une balise en porte).
    stream.next();
    return null;
  },
};
