# ws15-quality-hygiene — gouvernance qualité CI (terminé)

Quatre jobs CI (`.github/workflows/test.yml`) : `doc-lint` (`missing_docs`, cliquet
bloquant en régression, 6 crates non-cli, baseline 621), `unwrap_used`/`expect_used`
(pas de job dédié — les 6 crates non-cli sont directement en `#![deny(...)]`, le job à
cliquet temporaire `unwrap-lint` supprimé une fois tous propres), `clippy-pedantic`
(non bloquant, baseline 585), `coverage` (première mesure sans seuil, 74,97 % lignes,
push `main` seulement). Détails techniques complets : `docs/architecture.md` section
"Gouvernance qualité — jobs CI (WS-15)". `docs/features/ws15-quality-hygiene.md`
supprimé à la clôture. 18 commits, 548 tests, 0 régression à chaque étape.

**Le design V1 (produit par un autre agent, "laguna s2.1") était truffé de chiffres
fabriqués** — pas juste imprécis, carrément faux et non vérifiables : "0 test" pour
`tools`/`controller` (mesurés à 73/65), "2131 tests" sur `sandbox` seul (total workspace
réel : 548), "~175 unwrap() en production" (mesuré à 35, et les 5 exemples cités à
l'appui pointaient vers du code de test, pas de la production), un ratio doc "lignes
`///` / items pub" qui dépasse 100% sur 3 crates sur 5. Le crate `crds` (7ᵉ membre du
workspace) n'apparaissait dans aucun relevé. Confirme la valeur de relire un design
produit par un autre agent commande par commande avant de l'utiliser comme base — cf.
`docs/architecture.md` pour les chiffres corrigés et reproductibles.

**Trois pièges techniques vérifiés empiriquement en cours de route** (détail complet et
commandes dans `docs/architecture.md`, section dédiée) :
- `#![warn(X)]` en source a une précédence absolue sur tout flag CLI `-A`/`-D` — a cassé
  le job `clippy` par défaut (`-D warnings`) pendant plusieurs commits sans être détecté,
  faute d'avoir rejoué la vraie commande CI après chaque tâche (`cargo check` seul ne
  suffit pas).
- `cargo check`/`cargo test` n'exécutent **jamais** les lints clippy — un
  `#![deny(clippy::X)]` peut être silencieusement cassé par du code de test sans que ni
  l'un ni l'autre ne le détecte. Seul `cargo clippy --workspace --all-targets -- -D
  warnings` (la commande CI réelle) fait foi.
- La compilation incrémentale de rustc/cargo sous-compte les diagnostics de façon non
  déterministe selon l'état du cache (`missing_docs` : 109 vs 169 réel sur `sandbox` ;
  pedantic+nursery : 583 vs 959 réel) — `CARGO_INCREMENTAL=0` obligatoire pour toute
  mesure de warnings, en local comme en CI.

**Nouveau mode d'échec Qwen, spécifique à cette feature** : le modèle sous-jacent
(context natif 262 144 tokens ; vLLM plafonné à `--max-model-len 131072` jusqu'au
2026-08-02, corrigé depuis — cf. `docs/architecture.md` section "Limite d'outillage")
peut échouer par compaction de contexte sur une tâche qui touche beaucoup de fichiers
volumineux, même bien spécifiée — la session se compacte en
cours de route et finit par poser une question au lieu d'agir (pas un appel d'outil
bloqué par `question: deny`, juste du texte de fin de tour ; le résumé post-compaction
peut aussi halluciner des détails, ex. citer des crates "frontend"/"harness-core"
inexistants dans ce contexte). Une tâche combinant `sandbox`+`controller` (~6000 lignes
à lire) a échoué deux fois avant scindage par crate (task-05a/05b). **Quand le contrat
d'une tâche est déjà entièrement écrit et le risque de récidive élevé, appliquer
directement les modifications (Claude, via Edit) plutôt que de multiplier les tentatives
de délégation est plus efficace** — pas un problème de spécification qu'une réécriture
peut résoudre, une limite matérielle de l'outil. Seule feature du projet où Claude a
appliqué du code directement plutôt que de déléguer à Qwen, et le contexte explique
pourquoi (pas un contournement du workflow, une réponse à un blocage réel constaté deux
fois de suite).
