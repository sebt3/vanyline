# Feature — ws10-language-support

## Ce que la feature fait

Détecte l'usage de **Rust** et **JavaScript/TypeScript** dans les projets
(au clonage initial et à chaque maintenance), stocke le résultat dans le status
du Project, en dérive automatiquement les toolchains des sandboxes, et fournit
un tool `validate` qui lance test/lint/formatage pour les toolchains détectées
en ne remontant au LLM que les problèmes et les statistiques de succès.

## Ce qu'elle ne fait pas

- Aucun autre langage que Rust et JS/TS — et ce périmètre restera longtemps
- Pas d'affichage frontend des résultats (l'endpoint suffit ce sprint)
- Pas de persistance des résultats de validation (mémoire sandbox — décision
  2026-07-12 ; pod redémarré = relancer `validate`)
- Pas de gate/blocage : `validate` informe, il n'empêche rien

## Détection (via WS-9)

`vanyline-maint detect --workspace <dir>` inspecte le clone bare (arbre de HEAD) :

| Langage | Marqueurs |
|---------|-----------|
| `rust` | `Cargo.toml` (racine ou workspace members) |
| `js-ts` | `package.json` (racine) ; `tsconfig.json` compte aussi |

Sortie : JSON `{"languages": ["rust", "js-ts"]}` sur stdout.

**Remontée au status** : `vanyline-maint` patche directement le **status
subresource** du Project via l'API K8s (le pod de job tourne déjà avec un SA —
ajouter le RBAC `projects/status: patch` au Role du job). Alternative rejetée :
parser les logs du pod depuis le controller (fragile) ou fichier sur PVC relu
par le reconciler (indirection inutile). `detect` est appelé en fin d'`init` et
de `fetch` (la maintenance périodique rafraîchit la détection).

Nouveau champ : `ProjectStatus.languages: Vec<String>` (+ `detected_at`).

## Toolchains automatiques

Dans le reconciler Sandbox : si `sandbox.spec.toolchains` est vide, dériver de
`project.status.languages` — `rust` → image rust (trixie), `js-ts` → image node
(trixie), mêmes presets d'env qu'aujourd'hui. Une spec explicite garde la
priorité (comportement actuel inchangé). Les images par défaut par langage sont
des constantes du controller surchargables par env (`TOOLCHAIN_IMAGE_RUST`,
`TOOLCHAIN_IMAGE_NODE`).

## Tool `validate` (crate tools) et rapport

`validate` (nouvelle entrée de `tools/src/mcp.rs` + module `tools/src/validate.rs`) :

- Par toolchain détectée, exécute (via `command::execute`, timeouts généreux) :
  - **rust** : `cargo fmt --check`, `cargo clippy --workspace`,
    `cargo test --workspace`, et si la couverture est demandée
    `cargo llvm-cov --lcov --output-path <tmp>`
  - **js-ts** : les scripts de `package.json` s'ils existent — `format`/`lint`/
    `test` (convention assumée : on lance ce que le projet déclare, on ne
    devine pas l'outil) ; coverage si le script `test` sait produire du lcov
    (`coverage/lcov.info` conventionnel)
- Parse les sorties en `ValidationReport` :

```rust
pub struct ValidationReport {
    pub toolchain: String,              // "rust" | "js-ts"
    pub steps: Vec<StepResult>,         // { step, passed, problems: Vec<String> (bornés) }
    pub stats: Stats,                   // tests passés/échoués, problèmes lint, etc.
    pub coverage: Option<CoverageSummary>, // % global + % par fichier (parse lcov)
}
```

- **Retour au LLM** : uniquement les problèmes (bornés, tête+queue comme
  tools-v2) et les stats — jamais la sortie brute complète d'un run vert.

**Où vit l'état** : la crate tools reste pure (elle *retourne* le rapport) ;
c'est le serveur sandbox qui le range dans son état (`RwLock<Option<…>>` par
type de rapport) et l'expose sur `GET /validate/results` (JSON : rapports +
lcov brut en champ séparé — Codecov-compatible). Le tool MCP `validate` de la
sandbox = glue tools → état → réponse LLM.

Le CLI expose aussi `validate` en local tool (sans endpoint — le rapport part
dans la réponse au LLM, c'est tout).

## Risques et questions ouvertes

- **`cargo-llvm-cov` n'est pas dans l'image toolchain rust** : l'embarquer en
  binaire dans l'image sandbox de base (préféré — déterministe) ou
  `cargo install` au premier usage dans le CARGO_HOME du PVC (lent une fois).
  À trancher en tâche image ; le reste de la feature n'en dépend pas
  (coverage = optionnel).
- **RBAC du patch de status par le job** : à valider sur cluster réel tôt
  (c'est le seul point d'infrastructure nouveau de la détection).
- La convention "scripts package.json" peut donner un `validate` vide sur un
  projet JS sans scripts — acceptable, le rapport le dit explicitement.
- Workspaces Cargo imbriqués (comme vanyline lui-même) : `detect` marque `rust`
  dès qu'un Cargo.toml existe, `validate` s'exécute à la racine du worktree —
  suffisant v1.

## Découpage en tâches candidates

1. `detect` — implémentation dans `vanyline-maint` + patch de status + RBAC
   (validation cluster incluse)
2. `status-crd` — champ `ProjectStatus.languages`, régénération CRDs
3. `auto-toolchains` — dérivation dans le reconciler Sandbox + tests
4. `validate-rust` — runner rust + parsing + `ValidationReport` (tests sur
   fixtures de sorties cargo)
5. `validate-js` — runner js-ts (convention scripts) + tests
6. `validate-coverage` — parse lcov → `CoverageSummary` + décision
   cargo-llvm-cov dans l'image
7. `sandbox-endpoint` — état en mémoire + `GET /validate/results` + tool MCP
8. `cli-local-tool` — `validate` dans `cli/src/tools.rs`
