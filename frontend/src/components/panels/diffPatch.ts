/** Ligne parsée d'un hunk : son marqueur et son contenu (sans le préfixe). */
interface HunkLine {
  marker: ' ' | '-' | '+';
  content: string;
}

/** Parse un patch unifié et retourne les hunks avec leurs marqueurs et contenus.
 *  Lignes `\ No newline` sont exclues. */
function parseHunks(patch: string): { newStart: number; lines: HunkLine[] }[] {
  const lines = patch.split('\n');
  const hunks: { newStart: number; lines: HunkLine[] }[] = [];
  let i = 0;

  while (i < lines.length) {
    const m = lines[i].match(/^@@ -(\d+)((?:,\d+)?) \+(\d+)((?:,\d+)?) @@/);
    if (!m) {
      i++;
      continue;
    }

    const newStart = parseInt(m[3], 10);
    const hunkLines: HunkLine[] = [];
    i++;

    while (i < lines.length) {
      if (/^@@ /.test(lines[i])) break;
      if (/^diff |--+|^\+\+\+/.test(lines[i])) break;

      const ch = lines[i][0];
      if (ch === ' ' || ch === '-' || ch === '+') {
        hunkLines.push({
          marker: ch as ' ' | '-' | '+',
          content: lines[i].slice(1),
        });
      }
      // `\ No newline` ou autres → ignoré

      i++;
    }

    hunks.push({ newStart, lines: hunkLines });
  }

  return hunks;
}

/** Applique un patch unifié git en sens INVERSE sur `working` (le contenu
 *  courant du fichier, lu via /ws/fs) pour reconstruire le contenu de base
 *  (a). Format attendu : sortie brute de `git diff` (en-têtes `diff --git`,
 *  `index`, `---`, `+++` ignorés ; hunks `@@ -l,s +l,s @@` ; lignes de
 *  contexte ` `, suppressions `-`, ajouts `+`, marqueur `\ No newline at end
 *  of file`). `patch` vide ou sans hunk → retourne `working` tel quel
 *  (fichier untracked / aucun changement).
 *
 *  Positions : le hunk décrit les positions dans `working` (`+<start>` est
 *  1-indexé). Les hunks sont appliqués de la FIN vers le DÉBUT (les
 *  positions sont relatives au document `working` original — une application
 *  dans l'ordre décalerait les positions suivantes).
 */
export function reconstructBase(working: string, patch: string): string {
  if (!patch) return working;

  const hunks = parseHunks(patch);
  if (hunks.length === 0) return working;

  // Appliquer les hunks de la FIN vers le DÉBUT pour préserver les positions
  const sorted = [...hunks].sort((a, b) => b.newStart - a.newStart);
  let doc = working;

  for (const hunk of sorted) {
    // Position 0-indexée de départ dans working
    let pos = hunk.newStart - 1;

    const lines = doc.split('\n');

    let ok = true; // pour comportement dégradé sans panique
    for (let j = 0; j < hunk.lines.length; j++) {
      const hl = hunk.lines[j];

      if (pos < 0 || pos > lines.length) {
        ok = false;
        break;
      }

      // Regarder si le prochain est un + (indique remplacement)
      const nextHl = hunk.lines[j + 1];
      const isSwap = hl.marker === '-' && nextHl && nextHl.marker === '+';

      switch (hl.marker) {
        // Contexte : la ligne existe déjà, avancer
        case ' ':
          pos += 1;
          break;

        // Ligne `-` (absente de working) :
        //   → si le suivant est `+` : remplacement (`+` sera skip)
        //   → sinon : insertion pure
        case '-': {
          if (isSwap) {
            // Remplacement : la ligne en pos contient la version ancienne
            // (ou nouvelle, selon l'état de `working`) → on la remplace
            // par l'autre version pour reconstruire la base.
            if (lines[pos] === nextHl!.content || lines[pos] === hl.content) {
              // Si on a la version `+content`, la remplacer par `-content`
              // (working a la version modifiée → on veut la version base)
              // ou inversement selon le cas.
              lines[pos] = lines[pos] === nextHl!.content
                ? hl.content  // +version → -version
                : nextHl!.content; // -version → +version
            }
            pos += 1;
            j += 1; // skip le `+` suivant
          } else {
            // Insertion pure : le contenu `-` était dans la base,
            // pas dans working
            lines.splice(pos, 0, hl.content);
            pos += 1;
          }
          break;
        }

        // Ligne `+` (présente dans working) : supprimer à position courante
        // Le `+` est déjà skip si isSwap ; ici on ne l'atteint que
        // pour un `+` isolé (ajout pur du working → à retirer pour la base).
        case '+': {
          if (lines[pos] === hl.content) {
            lines.splice(pos, 1);
            // Ne pas avancer : la ligne suivante prend la place
          } else {
            ok = false;
          }
          break;
        }
      }
    }

    if (ok) {
      doc = lines.join('\n');
    }
    // Si pas ok → retourne working tel quel (dégradé sans panique)
  }

  return doc;
}