# Feature — ws15-quality-hygiene

## Ce que la feature fait

Unification de la gouvernance qualité du code — documentation, `unwrap()`/`expect()` en
production, clippy pedantic et une première mesure de coverage — via des gates CI, chacun
adossé à un chiffre **mesuré directement par l'outillage** (compilateur/clippy réels), pas
estimé par un script proxy. C'est une feature d'infrastructure/hygiène : aucun nouveau
comportement produit.

Quatre sous-items :

1. Documentation des items publics — lint natif `missing_docs`, gate à cliquet
2. `unwrap()`/`expect()` en production — élimination directe (le volume réel est faible)
3. Clippy pedantic/nursery — job non-bloquant à cliquet
4. Coverage — première mesure publiée, sans seuil (aucune baseline n'existe encore)

## Ce qu'elle ne fait pas

- Pas de refactor fonctionnel des fonctions longues — hors-scope, non traité ici.
- Pas de migration du logger projet ni de changement de convention `println!`/`eprintln!`
  côté `cli/` (exception déjà actée dans AGENTS.md pour un outil terminal).
- Pas de lint `missing_docs` sur `cli/` — même raison.
- Pas de seuil de régression coverage — il n'existe aucune mesure antérieure à comparer.
  Ce sera l'objet d'une feature suivante une fois qu'on a au moins deux points de mesure.
- Pas de scripts `grep`/`awk` maison pour approximer ce que le compilateur/clippy mesurent
  déjà exactement (voir "Révision" ci-dessous).

## Révision de la V1 du design (2026-08-01)

Une première version de ce document (produite par un autre agent, "laguna s2.1") a été
relue et son relevé vérifié commande par commande. Écarts trouvés, à titre d'archive —
**ne pas reprendre les chiffres de cette ancienne version** :

- « `tools` et `controller` : 0 test » — faux, mesurés à 73 et 65 tests respectivement.
- « `sandbox` = 2131 tests » — faux, mesuré à 135 (total workspace mesuré : 548, pas
  un multiple compatible avec 2131 sur un seul crate).
- « ~175 unwrap() en code de production » — faux, mesuré à 33 (voir tableau ci-dessous).
  Les 5 exemples cités à l'appui (`lib/src/domain.rs`, `event.rs`, `builtin/skill.rs`)
  pointaient tous vers des lignes situées **après** le `#[cfg(test)]`/`mod tests` du
  fichier — donc dans des `#[test] fn` qui ne retournent pas `Result`, où `?` ne
  compile pas.
- Ratio "lignes `///` / items pub" comme proxy de couverture doc — dépasse 100% sur
  3 crates sur 5 sans qu'aucune doc ne manque nécessairement (un item peut avoir un
  commentaire `///` de 5 lignes). Remplacé ici par le lint natif `missing_docs`, qui
  compte les items **réellement non documentés**, un par un.
- Le crate `crds` (7ᵉ membre du workspace depuis WS-12) n'apparaissait dans aucun
  relevé, seuil ou tâche du document V1.
- Contradictions internes (job "warn, non fail" décrit ensuite comme "échoue si...")
  et un chemin d'implémentation `tools/coverage/*.sh` qui collisionne avec le crate
  Rust `tools/`.

## Référentiel (mesuré le 2026-08-01, commandes reproductibles)

### Tests réels par crate

`cargo test --workspace` (sommé par crate, tests unitaires + fichiers `tests/`) :

| Crate | tests |
|---|---|
| cli | 127 |
| app | 61 |
| controller | 65 |
| crds | 5 |
| lib | 82 |
| sandbox | 135 |
| tools | 73 |
| **total** | **548** |

### `unwrap()`/`expect()` en production

Deux méthodes utilisées, qui doivent converger. D'abord un grep source (compté **avant**
le premier marqueur `#[cfg(test)]` de chaque fichier, hors tests inline et hors `tests/`,
`cli/` exclu — exception actée, cf. AGENTS.md) :

```bash
for f in $(find <crate>/src -name '*.rs'); do
  start=$(grep -n '#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1)
  if [ -z "$start" ]; then grep -c '\.unwrap()\|\.expect(' "$f"
  else head -n $((start-1)) "$f" | grep -c '\.unwrap()\|\.expect('; fi
done
```

Puis validé par la mesure **faisant foi** — le lint clippy réel qui posera le gate,
`clippy::unwrap_used`/`clippy::expect_used`, avec `--no-deps` **obligatoire** :

```bash
CARGO_INCREMENTAL=0 cargo clippy -p <crate> --lib|--bin <nom> --no-deps -- -W clippy::unwrap_used -W clippy::expect_used
```

**`--no-deps` est obligatoire** : `controller` dépend de `crds` (path dependency) — sans
`--no-deps`, `cargo clippy -p vanyline-controller -- -W clippy::unwrap_used` recompile et
compte AUSSI les warnings de `crds` (3), gonflant le total de `controller` de 11 à 14 (double
comptage avec la ligne `crds` du tableau). Trouvé en préparant task-03, avant qu'un mauvais
chiffre soit committé dans un job CI (contrairement au piège `CARGO_INCREMENTAL`, cf.
"Risques", qui lui avait déjà été committé une fois par erreur en task-01). `cargo rustc -p
<crate> -- -W missing_docs` (utilisé pour la documentation) n'a **pas** ce problème — vérifié
explicitement : `controller` donne 39 avec et sans `--no-deps`, donc task-01/task-02 ne sont
pas affectées. C'est spécifique à l'invocation `cargo clippy` avec des `-W` en trailing args,
qui se propagent au graphe de dépendances du même workspace, contrairement à `cargo rustc`.

| Crate | unwrap/expect en production (clippy, `--no-deps`, faisant foi) |
|---|---|
| lib | **0** |
| tools | **0** |
| crds | 3 |
| sandbox | 9 |
| app | 12 |
| controller | 11 |
| **total (hors cli)** | **35** |

### Documentation manquante — lint natif `missing_docs`

`CARGO_INCREMENTAL=0 cargo rustc -p <crate> --lib|--bin <nom> -- -W missing_docs`, compte
réel des items publics non documentés (pas un ratio) :

| Crate | items pub non documentés |
|---|---|
| controller | 39 |
| app | 161 |
| crds | 38 |
| tools | 60 |
| sandbox | 169 |
| lib | 154 |
| **total (hors cli)** | **621** |

**Piège trouvé en exécutant task-01, puis à nouveau en tentant task-02** : une première
mesure à cache incrémental tiède sous-comptait plusieurs crates — `sandbox` 109 au lieu de
169, `app` 7 au lieu de **161**, `controller` 1 au lieu de **39**. La compilation
incrémentale de rustc peut sous-compter les diagnostics selon l'état du cache ; seule une
mesure avec `CARGO_INCREMENTAL=0` (ou `cargo clean -p <crate>` préalable) est fiable. Détail
dans "Risques et questions ouvertes" — s'applique à toute mesure par comptage de warnings
dans cette feature (`clippy-pedantic-ratchet` inclus). Conséquence concrète : le plan
initial de task-02 ("`app`/`controller` quasi propres, `deny` immédiat") reposait sur les
chiffres sous-comptés et était faux — `missing_docs` s'applique à toute la crate (tous les
`mod` importés depuis `main.rs`), pas seulement au fichier édité ; `app` et `controller`
rejoignent donc le même traitement `warn` + cliquet que `lib`/`sandbox`/`tools`/`crds`,
il n'y a plus de "quickwin" séparé. Qwen a correctement remonté ce blocage plutôt que de
forcer un `deny` qui aurait cassé la compilation — cf. task-02 réécrite.

### Clippy pedantic + nursery

`CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -W clippy::pedantic -W
clippy::nursery` (build propre, `cargo clean` préalable) : **959 warnings** (style
uniquement, 0 error). Une première mesure sans ce soin (cache incrémental tiède, plusieurs
builds différents déjà effectués dans le même `target/`) avait donné 583 — sous-compté de
376, même cause que pour `missing_docs` ci-dessus. Clippy par défaut (`-D warnings`, gate CI
actuel) : **0 warning, job vert au 2026-08-01 avant task-01** — cassé par les attributs
`#![warn(...)]` posés ensuite en source (task-01/02/03), cf. "Risques" (finding critique).
Restauré par la tâche de correction dédiée avant task-05.

### Coverage

**Aucune mesure n'existe.** `cargo-llvm-cov` n'est pas installé localement ni en CI ; pas de
`cargo-tarpaulin` non plus. Choix d'outil : `cargo-llvm-cov` (composant `llvm-tools`
officiel du toolchain, plus rapide et mieux maintenu que `tarpaulin` — à reconfirmer au
moment de la tâche si l'écosystème a bougé). Aucun chiffre n'est avancé ici : la tâche 7
installe l'outil et publie la première mesure, qui devient la référence pour une feature
de suivi ultérieure.

## Documentation des items publics

Principe : lint natif du compilateur `missing_docs`, mesuré via `-W missing_docs` en ligne
de commande par le job CI `doc-lint` sur `lib`, `app`, `sandbox`, `tools`, `controller`,
`crds`. `cli/` exclu. `missing_docs` s'applique à **toute la crate** (tous les modules
atteignables depuis la racine), pas seulement au fichier visé — aucun des 6 crates n'est
assez propre pour un `#![deny(...)]` immédiat une fois mesuré correctement (39 à 169 items
manquants chacun). **Pas d'attribut `#![warn(missing_docs)]` en source** — testé et retiré
après task-04 (cf. "Risques", finding critique) : un tel attribut a une précédence
absolue sur tout flag `-A`/`-D` de ligne de commande, ce qui casse le job `clippy` par
défaut (`-D warnings`) dès qu'il est présent. Le job `doc-lint` reste pleinement
fonctionnel sans lui, puisqu'il active le lint lui-même via `-W missing_docs` à chaque
invocation. Un seul job CI à cliquet pour les 6 crates, qui **échoue en cas de
régression** au-delà de la baseline mesurée (621 au global — voir tâches). N'exige pas de
combler le passif tout de suite ; empêche seulement d'en ajouter. Combler le passif
(documenter les 621 items) est laissé pour une feature de suivi, crate par crate, une fois
le gate en place.

### Implémentation
- Job CI `doc-lint` (une seule fois, task-01, étendu par task-02) : compte par crate via
  `cargo rustc -p <crate> --lib|--bin <nom> -- -W missing_docs`, avec `CARGO_INCREMENTAL=0`
  obligatoire (sinon comptage non déterministe, cf. "Risques"). Échoue si le total dépasse
  la baseline (621). Pas de script séparé : le compilateur fait le calcul, le job ne fait
  que compter sa sortie.

## `unwrap()`/`expect()` en production

Principe : le volume réel (35, hors cli) est assez faible pour être traité directement,
sans script d'audit ni seuil dégressif sur plusieurs sprints. `lib` et `tools` sont déjà à
zéro — le lint `#![deny(clippy::unwrap_used, clippy::expect_used)]` peut y être posé
immédiatement, sans aucune correction préalable (avec `#[allow(...)]` sur le module de
test, cf. squelette ci-dessous).

Squelette de lint (à répliquer par crate, module de test exempté) :

```rust
// en tête de lib.rs / main.rs
#![deny(clippy::unwrap_used, clippy::expect_used)]
```
```rust
// dans le module de test concerné
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    // ...
}
```

Remplacement par occurrence : `?` si la fonction retourne déjà un `Result` compatible
(propagation réelle, à privilégier). **`clippy::expect_used` est deny au même titre que
`clippy::unwrap_used`** — remplacer un `.unwrap()` par un `.expect("raison")` ne suffit
PAS à satisfaire le gate (trouvé en écrivant task-04 : les deux lints sont posés
ensemble dès le départ, précisément pour empêcher ce contournement). Quand `?` n'est
vraiment pas possible (signature externe imposée, trait comme `Drop` qui ne peut pas
retourner `Result`, construction authentiquement infaillible), la seule option est un
`#[allow(clippy::unwrap_used)]`/`#[allow(clippy::expect_used)]` **local et documenté**
(commentaire d'une ligne expliquant pourquoi), pas un `.expect(...)` nu qui resterait
bloqué par le `deny`.

**Sur l'attribut de lint lui-même : `#![warn(...)]` en source ne doit jamais être posé
au niveau crate pour ces lints** (cf. "Risques", finding critique découvert en validant
task-04) — il casserait le job `clippy` par défaut (`-D warnings`) exactement comme
`#![warn(missing_docs)]` l'a fait. Les jobs à cliquet (`unwrap-lint`) mesurent via `-W`
en ligne de commande uniquement, sans attribut en source. Seul `#![deny(...)]` (l'état
final, une fois un crate propre) est sans risque, puisqu'il ne rentre jamais en conflit
avec `-D warnings`.

## Clippy pedantic/nursery — cliquet non bloquant

Le job `clippy` existant garde son rôle de gate bloquant (niveau défaut, `-D warnings`,
vert après la correction du finding critique — cf. "Risques"). Nouveau job
`clippy-pedantic`, **non bloquant** au sens où il
n'empêche pas le merge tant que le total ne régresse pas au-delà de la baseline mesurée au
moment de la tâche (959 au 2026-08-01, mesuré à froid — à remesurer à l'exécution, le
nombre bougera avec chaque ajout de code). Publie l'inventaire par catégorie en artifact.

### Implémentation
- `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --message-format=short -- -W clippy::pedantic -W clippy::nursery 2>&1 | grep -c ': warning: '`
  — `CARGO_INCREMENTAL=0` est **obligatoire**, pas optionnel (cf. "Risques" : sans lui, le
  comptage varie de plusieurs centaines selon l'état du cache).
- Job échoue si le total dépasse la baseline capturée à l'exécution de la tâche + une
  marge fixe (à définir dans la tâche — objectif : bloquer les régressions, pas imposer
  une décrue datée sur du contenu non encore mesuré).

## Coverage — première mesure, sans seuil

Principe : installer `cargo-llvm-cov`, publier un rapport par crate en artifact CI, **sans
gate**. Le seuil de régression (whatever threshold) n'a de sens qu'une fois une deuxième
mesure disponible — ce sera le contenu d'une feature de suivi, pas de celle-ci.

### Implémentation
- Job CI `coverage` (déclenché en push sur `main` seulement, coûteux) : installe
  `cargo-llvm-cov` (action `taiki-e/install-action@cargo-llvm-cov` ou équivalent),
  `cargo llvm-cov --workspace --json --output-path coverage.json` + résumé texte par
  crate, publié en artifact `coverage-report-<sha>`. Pas de comparaison, pas de fail.

## Découpage en tâches candidates

1. `doc-lint-baseline` — `#![warn(missing_docs)]` sur lib/sandbox/tools/crds + job CI
   `doc-lint` à cliquet (baseline mesurée à l'exécution, `CARGO_INCREMENTAL=0`
   obligatoire pour un comptage déterministe). **Terminé** — baseline réelle 421 (corrigée
   après coup : une première mesure à cache tiède avait donné 361, sous-comptée de 60 sur
   `sandbox` seul).
2. `doc-lint-extend-app-controller` — `#![warn(missing_docs)]` sur app/controller (mesuré
   161 + 39, pas de fix), étend le job `doc-lint` de task-01 pour couvrir les 6 crates,
   baseline globale portée à 621. Remplace l'ancien plan "quickwin deny immédiat" (basé sur
   une mesure sous-comptée à 7/1 — cf. "Documentation manquante" plus haut). **Terminé**,
   621 confirmé, 548 tests.
3. `unwrap-lint-baseline` — `#![deny(clippy::unwrap_used, clippy::expect_used)]` immédiat
   sur lib + tools (0 fix requis) ; `#![warn(...)]` + job CI à cliquet sur app/sandbox/
   controller/crds (baseline 35, `--no-deps` + `CARGO_INCREMENTAL=0` obligatoires,
   remesurée à l'exécution). **Terminé**, 35 confirmé, 548 tests.
4. `unwrap-fix-app-crds` — corriger les 15 occurrences réelles (app: 12, crds: 3), passer
   ces deux crates en `deny`. **Terminé**, 0 restant confirmé, 548 tests. 6 propagées via
   `?`/pattern `unwrap_or_else`+exit déjà établi dans le fichier, 6 `#[allow]` locaux
   documentés (cas authentiquement infaillibles ou contraints par une API externe) —
   `.expect("raison")` seul ne suffit pas, `expect_used` est deny au même titre que
   `unwrap_used` (cf. section "unwrap()/expect()" plus haut, corrigée après coup).
4bis. `fix-clippy-gate-warn-attributes` — **critique, non prévue au design initial**,
   découverte en validant task-04 : retirer tous les `#![warn(missing_docs)]` (6 crates,
   task-01/02) et `#![warn(clippy::unwrap_used, clippy::expect_used)]` (sandbox/
   controller restants, task-03) posés en source — ils cassent le job `clippy` par défaut
   (`-D warnings`) depuis le commit de task-01, jamais revérifié avec la vraie commande CI
   entre-temps. Cf. "Risques" pour le détail de la précédence rustc. Ne touche à aucun job
   CI (les jobs à cliquet sont autosuffisants via leur propre `-W`).
5. `unwrap-fix-sandbox-controller` — corriger les 20 occurrences réelles (sandbox: 9,
   controller: 11), passer ces deux crates en `deny`. **Terminé**, scindée en task-05a
   (sandbox) et task-05b (controller) après deux échecs de la version combinée (limite de
   contexte du modèle Qwen sous-jacent, 131K tokens, dépassée en lisant l'ensemble des
   fichiers sandbox — un vrai blocage d'outillage, pas un problème de spécification).
   Appliquée directement par Claude plutôt que déléguée, vu le contrat déjà entièrement
   écrit et le risque de récidive du même blocage. Les 6 crates non-cli sont maintenant
   tous en `deny` — le job à cliquet `unwrap-lint` (task-03) supprimé. `cargo clippy
   --workspace --all-targets -- -D warnings` vert sur tout le workspace, 548 tests.
6. `clippy-pedantic-ratchet` — job CI `clippy-pedantic` non bloquant, baseline remesurée
   à l'exécution **avec `CARGO_INCREMENTAL=0` et `cargo clean` préalable** (cf.
   "Risques" — sans ça la mesure du 2026-08-01 était sous-comptée à 583 au lieu de 959
   réels), artifact par catégorie.
7. `coverage-baseline` — installation `cargo-llvm-cov`, job CI `coverage`, rapport par
   crate en artifact, sans seuil.

## Risques et questions ouvertes

- **Découvert en exécutant task-05 — le modèle Qwen sous-jacent (context window 131K
  tokens) peut échouer par compaction de contexte sur une tâche qui touche beaucoup de
  fichiers volumineux**, même bien spécifiée. `task-05` (sandbox + controller combinés,
  ~6000 lignes de fichiers source à lire) a échoué deux fois : la session se compacte en
  cours de route et finit par poser une question au lieu d'agir (malgré `question: deny`
  dans la config de permissions de l'agent — ce n'est pas un appel d'outil bloqué par la
  permission, juste du texte de fin de tour). Scinder en tâches plus petites (task-05a/
  task-05b par crate) a réduit le risque sans l'éliminer complètement. Quand un contrat de
  tâche est déjà entièrement écrit (chaque changement précisé noir sur blanc, comme c'était
  le cas ici) et que le risque de récidive est élevé, appliquer directement les
  modifications plutôt que de multiplier les tentatives de délégation est plus efficace —
  ce n'est pas un problème de spécification que réécrire la tâche peut résoudre, c'est une
  limite matérielle de l'outil.
- **CRITIQUE, découvert en validant `task-04-fix-01` — `cargo check`/`cargo test` n'exécutent
  JAMAIS les lints clippy, y compris `#![deny(clippy::...)]`.** `clippy::unwrap_used`/
  `clippy::expect_used` sont des lints **clippy**, pas des lints rustc — `cargo check`/
  `cargo build`/`cargo test` utilisent rustc seul et ignorent silencieusement tout
  `#![deny(clippy::X)]` en source (aucune erreur, aucun warning, rien). Seul `cargo clippy`
  connaît et applique ces lints. Conséquence concrète : après avoir posé
  `#![deny(clippy::unwrap_used, clippy::expect_used)]` sur `app`/`crds` (task-04), la seule
  validation faite (`cargo check`/`cargo test`, 548 tests verts) ne pouvait STRUCTURELLEMENT
  PAS détecter que les modules `#[cfg(test)] mod tests { ... }` de 12 fichiers `app/` et de
  `crds/src/lib.rs` utilisent aussi `.unwrap()`/`.expect()`/`.unwrap_err()` dans leurs
  assertions — invisibles jusqu'à ce que `cargo clippy --workspace --all-targets -- -D
  warnings` (la vraie commande CI) soit rejouée pour valider `task-04-fix-01`. Fix :
  `task-04-fix-02`, ajoute `#![allow(clippy::unwrap_used, clippy::expect_used)]` dans les
  13 modules de test concernés (même pattern que task-03 avait déjà appliqué correctement
  sur `lib`/`tools`, jamais étendu à `app`/`crds` faute d'avoir tourné la bonne commande).
  **Leçon générale, definitive pour la suite de la feature** : toute tâche qui pose
  `#![deny(clippy::X)]`/`#![warn(clippy::X)]` doit être validée avec `cargo clippy
  --workspace --all-targets -- -D warnings` (la commande CI réelle, `--all-targets` inclus
  pour couvrir les tests), jamais seulement `cargo check`/`cargo test` — ces deux dernières
  commandes ne peuvent tout simplement pas voir un lint clippy, quel que soit son niveau.
- **CRITIQUE, découvert en validant task-04 — `#![warn(missing_docs)]`/`#![warn(clippy::
  unwrap_used, clippy::expect_used)]` posés en source (task-01/02/03) cassent le job CI
  `clippy` existant (`cargo clippy --workspace --all-targets -- -D warnings`), qui n'avait
  jamais été revérifié après ces tâches (seul `cargo check --workspace` l'avait été).**
  Rustc applique un ordre de précédence strict : un attribut en source (`#![warn(X)]`)
  l'emporte **toujours** sur un flag `-A X` en ligne de commande, quel que soit l'ordre des
  flags — vérifié empiriquement (`-D warnings -A missing_docs` et `-A missing_docs -D
  warnings` échouent tous les deux identiquement). `-D warnings` promeut ensuite tout
  warning actif (y compris ceux fixés par un attribut en source) en erreur. Résultat : le
  job `clippy` par défaut, censé rester vert et inchangé, échouait en fait avec ~100+
  erreurs dès le commit de task-01 — jamais détecté faute d'avoir rejoué la vraie commande
  CI après chaque tâche. **Fix (task de correction dédiée, avant task-05)** : retirer les
  attributs `#![warn(...)]` en source — les jobs à cliquet (`doc-lint`, `unwrap-lint`) n'en
  ont jamais eu besoin, ils utilisent déjà leur propre `-W` explicite en ligne de commande,
  autosuffisant. Les `#![deny(...)]` déjà posés (lib/tools pour unwrap, et maintenant app/
  crds après task-04) restent : `deny` ne pose pas ce problème, il n'entre jamais en
  conflit avec `-D warnings` (l'un et l'autre convergent vers "erreur", pas de précédence à
  arbitrer). Leçon générale : **après toute tâche qui ajoute un attribut de lint en
  source, rejouer explicitement chaque commande de CI existante telle quelle** (pas
  seulement `cargo check`/`cargo test`) — un attribut en source peut casser un gate qui
  semblait sans rapport.
- **Confirmé pendant l'exécution de task-01 — la compilation incrémentale de rustc/cargo
  sous-compte les diagnostics de façon non déterministe.** `cargo rustc -p vanyline-sandbox
  --lib -- -W missing_docs | grep -c ...` a donné 109 avec un cache incrémental tiède
  (state hérité de builds précédents avec d'autres combinaisons de flags dans le même
  `target/`) contre 169 avec un build propre (`CARGO_INCREMENTAL=0` ou `cargo clean -p` au
  préalable) — écart reproduit 3 fois dans les deux sens. Même phénomène, en pire, sur
  `clippy --workspace --all-targets -- -W clippy::pedantic -W clippy::nursery` : 583
  (cache tiède) contre 959 (build propre, `cargo clean` complet). **Toute mesure par
  comptage de warnings dans cette feature doit forcer `CARGO_INCREMENTAL=0`**, et les jobs
  CI correspondants doivent le faire aussi (`Swatinem/rust-cache@v2` conserve le cache
  entre les runs GitHub Actions — sans ce garde-fou, le comptage en CI serait aussi
  instable). `doc-lint` (task-01) le fait déjà après correction ; `clippy-pedantic-ratchet`
  (task-06) doit le faire dès l'écriture de la tâche, pas après coup.
- **Confirmé en préparant task-03 — `cargo clippy -p <crate> -- -W <lint>` (contrairement à
  `cargo rustc -p <crate> -- -W <lint>`) propage les flags de lint aux dépendances internes
  du workspace compilées dans la même invocation.** `controller` dépend de `crds` (path
  dependency) : `cargo clippy -p vanyline-controller -- -W clippy::unwrap_used` comptait 14
  (11 réels + 3 de `crds`, dépendance recompilée au passage) tant que `--no-deps` n'était
  pas passé. `--no-deps` est donc **obligatoire** pour toute mesure clippy par crate dans
  cette feature (`unwrap-lint-baseline`, `clippy-pedantic-ratchet`) — `missing_docs` n'est
  pas concerné (`cargo rustc`, pas `cargo clippy`, vérifié explicitement : 39 avec et sans
  `--no-deps` pour `controller`).
- **Les baselines (421 doc, 35 unwrap, 959 pedantic) sont mesurées le 2026-08-01, à froid,
  avec `CARGO_INCREMENTAL=0` et `--no-deps` où applicable** — tout commit entre cette date
  et l'exécution de chaque tâche peut les faire bouger. Chaque tâche remesure au moment de
  l'exécution plutôt que de coder en dur les chiffres de ce document ; ce document sert de
  point de départ, pas de vérité figée.
- **`missing_docs` sur `crds`** : c'est un crate généré en partie par `#[derive(CustomResource)]`
  (kube-derive) — vérifier que le lint ne compte pas de faux positifs sur du code généré
  par macro avant de figer la baseline dans la tâche 1.
- **`cargo-llvm-cov` non testé sur ce toolchain** (`rustc 1.96.1`) — la tâche 7 doit
  valider l'installation avant de considérer le job fiable ; si `llvm-tools` composant
  manque en CI (`dtolnay/rust-toolchain@stable` sans `components: llvm-tools-preview`),
  l'ajouter.
- **`#![deny(clippy::unwrap_used)]` crate-level peut avoir des faux positifs** dans du code
  macro-généré (ex: `#[derive(CustomResource)]` sur `crds`, dérivations `thiserror` sur les
  autres) — vérifier avant de poser le `deny`, `#[allow]` local documenté sinon.
- **Le job `clippy-pedantic` en `--message-format=short` peut changer de format entre
  versions de clippy** — si le comptage `grep -c ': warning: '` casse silencieusement (0
  au lieu du vrai total), le job passerait à tort. Prévoir une assertion basse (ex: échoue
  aussi si total < 50, signe que le parsing a cassé) dans la tâche 6.

## Validation attendue

- `cargo clippy --workspace --all-targets -- -D warnings` → 0 warning (gate conservé, déjà vert)
- job `doc-lint` → 0 régression sur les 6 crates (lib/app/sandbox/tools/controller/crds)
  par rapport à la baseline mesurée à l'exécution (621)
- `lib` et `tools` : `#![deny(clippy::unwrap_used, clippy::expect_used)]` actif dès la
  tâche 3, sans aucune correction de code nécessaire
- après tâches 4/5 : les 6 crates non-cli en `deny(unwrap_used, expect_used)`, 0 occurrence
- job `clippy-pedantic` → 0 régression par rapport à la baseline mesurée à l'exécution
- job `coverage` → rapport par crate publié en artifact (pas de gate)
