# Session du 2026-11-13 — specs du cœur, widgets, configuration, besoins de miryad-core

Notes de reprise. Le détail des décisions vit dans les `.sdd` et les DSD cités, pas ici : ce fichier dit **où reprendre** et
**pourquoi**. La session a surtout écrit des specs (rôle de Claude) ; aucun code de production n'a été touché.

## 1. État du dépôt à la fin de la session

Commits locaux (non poussés), tous de cette session :

| Commit | Contenu |
|---|---|
| `9fad06d` | Lot 1 — specs `dashboards` : clés d'écran par projet, grille figée à 12, `009`/`010`, défauts conformes à `AC-7` |
| `60a68e2` | Lot 2 — specs `sessions` / `workflow` (`default_profile`, `trigger "app"`, serveurs `forward`) |
| `29739c4` | Lot 3 — specs `main`/`config`/`auth`/`error`/`api`/`db`/`migration`, `lib/error`, règles dans `vanyline.sdd` et `tooling.sdd` |
| `1fae200`, `85e5c46` | DSD `dsd/app/widgets.md` |
| `191ebb9` | Specs `app/src/widgets/` + amendements (`registry`, `mod`, entité `dashboards`, `error`, `config`, `api/mod`, `main`, `catalog`) |

**NON COMMITÉ (à commiter à la reprise après relecture) :**
- `dsd/app/configuration.md` — DSD de la configuration, **jamais relu en entier par le développeur** ;
- `app/src/definitions/*.sdd` — 6 specs des miroirs ts-rs (`mod`, `agent`, `toolset`, `context`, `provider`, `rows`) ;
- `docs/chantier-lymaille.md` — pointeur `H2` ajouté (miroirs ts-rs).
`spikes/` est non versionné et **doit le rester** (voir §2).

`specdd lint` : 175 specs, 0 erreur, 0 warning.

## 2. Méthode — règles confirmées par le développeur

- **Les décisions vivent dans les `.sdd`** (ou le DSD), jamais dans un fichier d'arbitrage à côté. J'avais écrit un registre de
  réponses hors specs : c'était faux, supprimé. Un `[?]` traité disparaît de sa spec, remplacé par le contrat à l'état résultant.
- **`docs/chantier-lymaille.md` n'est qu'un ordonnancement** (dérive assumée du protocole, tolérée). On n'y met que des pointeurs
  vers les `Tasks` `[ ]` des specs, **jamais de décision**. Qwen exécute (tests, code) et remonte ses questions ; la résolution
  des questions est le travail de Claude, dans les specs.
- **Lire `.specdd/bootstrap.md` et `bootstrap.project.md` avant d'écrire une spec** (je ne l'avais pas fait au début de session).
  Une spec décrit l'état résultant d'un sujet, pas une tâche ; les `Tasks` sont locales.
- **Les spikes sont jetables** : on les supprime une fois la stratégie consignée dans la spec ou le DSD ; on ne les commite pas.
- **Ne pas imposer de restriction non demandée** : ni lecture seule des scripts de widgets (voulu : un widget peut agir), ni bornes
  arbitraires sur les scripts (les plafonds sont de la configuration d'application, larges par défaut).
- **Vérifier dans le code avant d'affirmer** : sources de miryad-core et lymaille dans `~/projets/miryad-core`, `~/projets/lymaille`
  (mêmes versions que les crates publiées) et `~/.cargo/registry`. Les questions de conception passent par `AskUserQuestion`.
- Périmètre des questions `[?]` traitées : app touchée par la migration + `lib.sdd`/`error.sdd`. Hors objet (code retiré ou non
  touché) : `cfgstore/`, `cli/`, `tools/`, `sandbox/`, `controller/`, `crds/`, `health/`, `api/{me,owners,projects,sandboxes}`,
  `ws/sandbox_state`, `k8s`, `owner_links`.

## 3. Décisions structurantes de la session

- **Tableaux de bord** (`app/dashboards`) : un écran sandbox se personnalise **par projet** (`project_id` FK `CASCADE`),
  `sandbox_id` informatif (`SET NULL`) ; grille fixe de 12 colonnes ; doublon `custom` refusé `VNL-APP-DSH-009` par le hook ;
  écran impossible 400 `VNL-APP-DSH-010` (`008` réservé au défaut illisible) ; `mermaid` et `scripted` au registre (12 types).
- **Widgets scriptés** (`dsd/app/widgets.md`) : script Rhai exécuté par le **`ScriptRunner` de miryad-core** (droits = `script_grants`
  du type de contexte ∩ principal), sous l'identité du spectateur ; la sortie du script est la déclaration d'un **widget graphique**
  (`bar`/`pie`, ECharts, JSON seulement) ; `markdown` et `mermaid` prennent leur contenu **directement** en paramètre ; cache par
  utilisateur (TTL 1 h par défaut, `0` = sans cache) ; plafonds `VNL_WIDGET_*` configurables ; état prêt / en cours / échec.
  **V1 : écran d'accueil seulement** (`G_home`), les contextes `H_…` n'ont pas de `script_grants` (miryad-core#70).
- **Configuration** (`dsd/app/configuration.md`) : écrans de formulaires (pas de tableaux de bord), tout le modèle lymaille d'un coup,
  une liste par type avec badge d'espace (Privé/Catalogue), référence de secret jamais de clé, transfert CLI hors périmètre.
  Types TS : **miroirs Rust dérivant `TS`, gardés par des tests de contrat** (`app/src/definitions/`), retirés quand miryad-core
  exporte (#72).
- **Réglage `default_profile`** remplace `default_model` (`app_settings`) ; le profil `"default"` **ne peut pas être semé** (un profil
  nomme un fournisseur inconnu au démarrage) : `[?]` ouverts dans `sessions/catalog.sdd` (voir §5).
- Règles transverses posées dans `vanyline.sdd` : on spécifie notre usage d'une dépendance, jamais le crate ; un espace de marqueurs
  `VNL-<X>-*` par crate, un marqueur par cause. Marqueurs créés : `VNL-CFG-007/009`, `VNL-AUTH-005`, `VNL-TEST-nnn`,
  `VNL-APP-API-001`, `VNL-APP-DSH-009..012`, `VNL-APP-WDG-001..010`.

## 4. Tâches en attente pour Qwen (`docs/chantier-lymaille.md`)

Rien n'est lancé. Ordre : **G4b** (alignement code/tests des specs `dashboards`, avant tout le reste) → **H1** (widgets, écran d'accueil)
et **H2** (miroirs ts-rs, avant les écrans de configuration) → G3/G6 (frontend du moteur de rendu, specs **non écrites**).
Les tâches de code du lot « cœur » (`main` : 404 JSON sous `/api`, shutdown gracieux, `cookie_key` retiré, marqueurs ; `config` : vide ≡
absent ; `error` : `K8sNotFound` typé ; `sessions` : `trigger`, `default_profile` ; `test_support` : pool unique) sont des `[ ]` **dans les
specs concernées**, pas dans le chantier.

## 5. miryad-core : une livraison est en cours côté développeur

**La décision « on construit avec ce que livre la stack, sans aller-retour vers miryad-core » est ABROGÉE (2026-11-13).** Le développeur
corrige miryad-core d'abord ; ticket de cadrage : https://github.com/sebt3/miryad-core/issues/83.

| Ticket | Sujet | Contournement vanyline à retirer / spec à revoir |
|---|---|---|
| #70 | `script_grants` des contextes `H_…` | widgets V1 limités à `G_home` (`widgets#OI-1`, `widgets/run.sdd`) |
| #71 | jeton d'annulation de `ScriptRunner::run` | interruption par abandon côté app (`widgets#OI-2`) |
| #72 | export ts-rs des définitions et lignes | `app/src/definitions/` (miroirs + tests de contrat) |
| #73 | publication système du catalogue, lignes gérées | SQL brut create-only de `sessions/catalog.sdd` |
| #74 | `context_type` `G_…` refusé par les hooks | `[ ]` de `catalog.sdd` |
| #75 | profil par défaut | `default_profile` d'`app_settings`, 2 `[?]` de `catalog.sdd` |
| #76 | identifiants `H_…` avec `-` | encodage `-`↔`_` de `sessions/host.sdd` |
| #77 | 404 uniforme, 409 sur contrainte unique | `main.sdd` (oracle), `db/entities/dashboards.sdd` (filet de course 500) |
| #78 | filtres multiples et tri de la liste REST | listes de tableaux complémentaires (G3) |
| #79 | `ToSchema` sur les `Model` de lymaille | dix fragments OpenAPI manuels de `main.sdd` |
| #80 | noms des `ctx.run`, client d'admin Restate | `workflow/status.sdd` (client réécrit, formats non vérifiés) |
| #81 | haute disponibilité des sessions | `app` mono-réplique (`app/sessions#DEC-1`) |
| #82 | jeton `forward` hors session | `widgets#OI-4`, `workflow/runtime.sdd` |
| #26 (ancien) | kinds de step durables | `sandbox/jobs.sdd`, kind `shell` |

**À la reprise :** (1) vérifier ce qui est livré (version de miryad-core / lymaille / vynil-core) ; (2) pour chaque ticket livré, relire les
specs de la colonne de droite et retirer le contournement (spec d'abord) ; (3) ajouter les numéros de ticket aux `[ ]` de `main.sdd`
(404 uniforme → #77 ; `ToSchema` → #79) et de `catalog.sdd` (`context_type` → #74), qui attendaient un ticket ; (4) si la version de
miryad-core change, le bump est une tâche de type A1/A2 (specs puis code), comme pour 0.3.1.

## 6. Questions encore ouvertes

- **`dsd/app.md` `FR-4`** : demande qu'un widget scripté produise « au minimum du markdown » ; la V1 le limite aux graphiques (le texte
  d'un widget `markdown` est un paramètre). Consigné en `widgets#OI-8`, **amendement de `app#FR-4` à confirmer**.
- `sessions/catalog.sdd` : 2 `[?]` (profil par défaut non semé ; suivi du réglage par les agents publiés) — se règlent avec #75.
- `error.sdd` : collision `VNL-SBX-002` (ticket de `api/sandboxes`, hors périmètre) ; `dashboards/registry.sdd` : besoins du widget
  `chat` (`OI-1` du DSD, à trancher avec les specs frontend).
- `widgets` : `OI-3` (`script_grants` de `G_home` : rien par défaut, donc **un widget ne voit rien tant qu'un administrateur n'a pas
  accordé de toolset**), `OI-4`, `OI-5` (widgets d'action, à concevoir), `OI-7` (déjà ratifié : validation Rust par type).
- `configuration` : `OI-1` (le `scaffold` de miryad génère du shadcn-vue, le frontend utilise Nuxt UI), `OI-2` (essais de connexion
  fournisseur/MCP sans route), `OI-4` (contextes et types de contexte : ce qu'un utilisateur ordinaire en fait).

## 7. Prochaines étapes (après la livraison miryad-core)

1. Commiter les fichiers en attente (§1) après relecture du DSD `configuration`.
2. Écrire les **specs frontend de la configuration** (client de ressources par id à la place de `httpConfigRepo`, un écran par ressource,
   `ConfigShell`, `SettingsView`) et celles du **moteur de rendu des tableaux** (G3 : `bar`/`pie` ECharts, `markdown`, `mermaid`, hôte
   de widget scripté à trois états) — les deux demandent d'abord de trancher `configuration#OI-1`.
3. Reste à faire côté controller : régler la rétention Restate (≥ 14 jours, `workflow/status.sdd`) ; vérifier les formats d'admin
   Restate sur un cluster réel.
4. Lancer Qwen sur G4b puis H1/H2, spec par spec, avec le `validator` ; cocher les `[ ]` seulement sur synthèse verte.
