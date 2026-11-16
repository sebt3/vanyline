# Solution Design: Écrans de configuration

## Résumé

La Solution `app/configuration` donne à l'utilisateur les écrans de la configuration de ses agents sur le modèle de lymaille et de
miryad-core : fournisseurs, profils de modèle, serveurs MCP, agents, toolsets (avec leurs skills), types de contexte, contextes, et le
réglage global du profil par défaut. Elle remplace les six écrans de `@vanyline/ui` bâtis sur l'ancien magasin de configuration de
vanyline, dont le backend est retiré, et les rebranche sur le CRUD REST générique de miryad-core (`/api/v1/<ressource>`). Elle reste en
**écrans de formulaires** (`app#OI-2`) ; le tableau de bord n'y sert qu'à personnaliser l'accueil de la configuration. L'agent de
configuration, présent à côté de chaque écran, écrit les mêmes ressources avec les mêmes droits.

Elle réalise `app#BR-3`, `app#FR-1` pour le contexte « configuration », et la part « écrans de configuration » de `app#CON-3`. Le transfert
du niveau partagé avec la CLI (`app#OI-7`) est une Solution à part.

## Problème et objectif

Les écrans actuels (fournisseurs, profils, MCP, toolsets, agents, skills) supposent un modèle qui n'existe plus : couches workspace/global,
fournisseur par défaut, clé d'API saisie dans l'écran, skills en ressource propre, clés par nom. Le modèle de lymaille est différent :
des définitions JSON, un espace privé et un catalogue `G_…`, des droits d'administrateur sur les fournisseurs, profils et serveurs MCP,
des types de contexte et des contextes qui n'ont pas d'écran. Objectif : un jeu d'écrans fidèle à ce modèle, sans réimplémenter ses
règles côté interface, et sans secret affiché ni stocké par l'application.

## Périmètre et non-objectifs

Couvert :

- les écrans par ressource, avec leur forme de formulaire, leur liste et leurs droits visibles ;
- le client de ressources du frontend (clés entières, mise à jour par fusion) qui remplace `httpConfigRepo` ;
- l'amorçage d'un compte sans profil de modèle utilisable ;
- le réglage global `default_profile` (`app/sessions`), l'écran du compte et la coque de configuration ;
- l'arrimage au tooling de miryad-core (IR des ressources, génération des écrans de liste, types).

Hors périmètre :

- les règles d'accès, de nommage (`G_`) et de validation des définitions : ce sont celles de miryad-core et de lymaille, appliquées par
  leurs hooks ; l'interface les reflète et n'en décide pas ;
- le transfert du niveau partagé avec la CLI (import et export sans secrets, `app#OI-7`) : Solution séparée ;
- la création des secrets eux-mêmes : une référence de secret est saisie, le secret vit dans le déploiement (`DEC-4`) ;
- l'édition par tableau de bord (`app#OI-2` : formulaires, décidé le 2026-11-13).

## Contexte applicable du domaine

- `app#BR-3` : un utilisateur configure par l'agent de configuration ou par les écrans, jamais par une session productive.
- `app#FR-1` : un chat avec un agent depuis chaque écran de contexte, dont la configuration.
- `app#CON-3` : écrans responsives, sauf l'IDE et le diff.
- miryad-core `lymaille#DEC-7`, `DEC-9`, `DEC-14`, `DEC-19`, `DEC-20` : espace privé et catalogue `G_…`, paires de ressources, qui écrit quoi.

## Acteurs

- **Utilisateur :** liste le catalogue, gère ses propres agents, toolsets et contextes, duplique une entrée du catalogue dans son espace.
- **Administrateur :** gère fournisseurs, profils, serveurs MCP, types de contexte et le catalogue (agents, toolsets, contextes partagés),
  et les réglages globaux. Il est reconnu par le groupe `admin` de `GET /api/v1/me`.
- **Agent de configuration :** écrit les ressources de l'utilisateur sous ses droits (`app#BR-3`).

## État actuel et état cible

- **Actuel (Observed in : `packages/ui/src/config/`, `frontend/src/components/SettingsView.vue`, `frontend/src/api/httpConfigRepo.ts`) :**
  six écrans (`ProvidersScreen`, profils, serveurs MCP, toolsets, agents, skills) dans `ConfigShell`, montés par `/settings` à côté du chat
  de l'agent de configuration, un écran « Global » (`app-settings`) et un écran du compte ; le port `ConfigRepo` traduit des noms en
  identifiants REST. Leur backend (`/api/llm-providers` et sœurs) est retiré ; D3 du chantier lymaille retire ces écrans.
- **Cible :** un écran par ressource de miryad-core montée par `app` (dix ressources de lymaille plus `app-settings`), sur le client de
  ressources ; `SettingsView` garde la coque, le chat et le rafraîchissement en fin de tour ; les écrans retirés par D3 sont remplacés,
  pas adaptés.

## Hypothèses, contraintes et dépendances

- **CON-1:** Les ressources sont celles que `app` monte : `lymaille_profiles`, `lymaille_providers`, `lymaille_mcp_servers`,
  `lymaille_agents` et `lymaille_toolsets` (privés, `OwnerOnly`), `lymaille_contexts` (privé), `lymaille_catalog_agents`,
  `lymaille_catalog_toolsets`, `lymaille_catalog_context_types`, `lymaille_catalog_contexts`, et `app-settings`.
    - **Source:** `app/src/main.sdd` ; miryad-core `lymaille#DEC-14`, `DEC-19`, `DEC-20`
- **CON-2:** Une ressource se désigne par un identifiant entier ; le nom vit dans la définition. Un identifiant du catalogue commence par
  `G_` et décide de la table ; un utilisateur n'en crée jamais.
    - **Source:** miryad-core `lymaille#DEC-9`
- **CON-3:** Les droits sont ceux de miryad-core, relus à l'écriture par des hooks asynchrones ; un refus se présente à l'utilisateur sans
  que l'interface ait tenté de le prévoir. L'interface ne masque que ce qu'elle sait d'avance : écriture du catalogue et des ressources
  d'administrateur réservée au groupe `admin`.
    - **Source:** miryad-core `lymaille#DEC-20`, `hooks-async#DEC-7`
- **CON-4:** Une référence de secret (`auth_ref`) est résolue par l'hôte (variable d'environnement par défaut) ; aucune valeur de secret ne
  transite par l'application ni l'interface.
    - **Source:** miryad-core `lymaille#DEC-16`
- **CON-5:** Les colonnes de définition (agent, profil, toolset, fournisseur) sont du JSON que l'IR de miryad-core présente comme un
  objet opaque ; leur forme interne est celle des types de lymaille. L'export ts-rs de miryad-core 0.3.1 ne couvre aujourd'hui que les
  événements de session (`SessionEvent`, `SessionEventKind`, `SessionOutcome`, par `export_typescript`) ; lymaille 0.2.1 n'en a aucun. Les
  définitions, les lignes de ressource et les corps de requête de session ne sont pas exportés.
    - **Source:** miryad-core `frontend.md` (IR), `src/lymaille/interactive/event.rs` ; lymaille `agents/`, `toolsets/` (vérifié le 2026-11-13)

## Exigences fonctionnelles

- **FR-1 - MUST:** Chaque type d'objet à deux espaces — agent, toolset, contexte — a **une seule liste** dont chaque ligne porte un badge
  d'espace : *Privé* ou *Catalogue*. Le catalogue est en lecture seule pour un utilisateur ordinaire et modifiable par un administrateur ;
  « Dupliquer vers mon espace » copie une entrée du catalogue dans l'espace privé de l'utilisateur, sous un nom sans préfixe `G_`.
    - **Source:** décision du développeur principal, 2026-11-13 ; miryad-core `lymaille#DEC-7`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Les ressources d'administrateur — fournisseurs, profils, serveurs MCP, types de contexte — ne sont proposées qu'aux
  membres du groupe `admin` (relu de `GET /api/v1/me`) ; un utilisateur ordinaire ne voit pas ces écrans. Les droits réels restent ceux
  du serveur : un 403 est présenté comme « droits insuffisants », jamais comme une panne.
    - **Source:** `CON-3`
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** Chaque type de ressource a un formulaire propre à la forme de sa définition : fournisseur (type, point d'accès,
  référence de secret), profil de modèle, serveur MCP (URL, délais, mode d'authentification, groupes autorisés), agent (nature, profil,
  toolsets accordés, agents délégables), toolset (source Rhai, paramètres, rôles MCP, skills), type de contexte (nature, agent par défaut,
  agents autorisés, `script_grants`), contexte. La mise à jour envoie une ligne **complète** fusionnée avec la ligne courante ; un champ
  vidé est envoyé vidé, jamais omis.
    - **Source:** `CON-5` ; `packages/ui/src/config/config-screens.sdd` (règle des champs vidés)
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** L'écran d'un fournisseur ne saisit et n'affiche jamais de valeur de secret : il saisit une **référence de secret**
  (et son mode), jamais une clé d'API. Un secret absent du déploiement se signale par l'échec d'usage, pas par une valeur lue.
    - **Source:** `CON-4`
    - **Verified by:** `AC-4`

- **FR-5 - MUST:** L'écran d'un toolset édite la source Rhai dans un éditeur de code (CodeMirror), ses skills, ses paramètres et ses
  rôles MCP ; la source active et le brouillon sont distincts (`lymaille#DEC-18`). Les skills n'ont pas d'écran propre : elles suivent
  leur toolset.
    - **Source:** miryad-core `lymaille#DEC-18`, `DEC-9`
    - **Verified by:** `AC-3`

- **FR-6 - MUST:** Un compte sans profil de modèle utilisable met l'agent de configuration à l'état « indisponible » et oriente
  l'utilisateur ordinaire vers son administrateur, et l'administrateur vers la création d'un fournisseur et d'un profil. Le réglage global
  `default_profile` (`app/sessions`) est édité dans l'écran « Global », réservé aux administrateurs.
    - **Source:** `app/sessions#FR-5` ; `app/src/db/entities/app_settings.sdd`
    - **Verified by:** `AC-5`

- **FR-7 - MUST:** Une configuration modifiée par l'agent de configuration rafraîchit les écrans ouverts à la fin du tour. Les actions de
  l'agent restent visibles dans la conversation.
    - **Source:** `frontend/src/components/SettingsView.sdd` (comportement conservé)
    - **Verified by:** `AC-6`

- **FR-8 - MUST:** Le client de ressources du frontend remplace `httpConfigRepo` : clés entières, cache nom ↔ identifiant par type, mise à
  jour par fusion de la ligne courante, erreurs de hook présentées avec leur code `MRD-*`.
    - **Source:** `frontend/src/api/httpConfigRepo.sdd` (règle de fusion conservée)
    - **Verified by:** `AC-3`

- **FR-9 - MUST:** Les écrans sont responsives (`app#CON-3`) et utilisables à 360 px ; ils suivent les règles de défilement et de densité du
  frontend (`frontend/src/layout.sdd`).
    - **Source:** `app#CON-3`
    - **Verified by:** `AC-7`

## Sécurité

- **Secrets.** Aucun secret n'est saisi, affiché ni journalisé (`FR-4`). Un champ de référence de secret n'est pas un champ secret, mais une
  valeur de configuration lisible.
- **Droits.** L'interface ne s'arroge aucun droit : elle reflète le groupe `admin` et présente les refus du serveur (`CON-3`). Le catalogue
  n'est jamais écrit par un non-administrateur, même si l'interface se trompait de bouton, parce que les hooks refusent.
- **Contenu.** Une source de toolset est du code Rhai que l'éditeur affiche et ne fait jamais exécuter à l'interface ; son exécution est
  celle de lymaille, avec ses grants.

## Décisions

- **DEC-1 - Des formulaires, pas des tableaux de bord**
    - **Decision:** les écrans de configuration restent des écrans de formulaires ; l'accueil de la configuration peut être un tableau
      personnalisable (`app/dashboards`), l'édition d'une ressource ne l'est pas.
    - **Alternatives:** tout en tableaux de bord (écarté : il faudrait inventer des widgets d'édition et protéger la configuration par le
      moteur de tableau).
    - **Source:** décision du développeur principal, 2026-11-13 (`app#OI-2` : « mixte, les formulaires pour éditer »)

- **DEC-2 - Tout le modèle d'un coup**
    - **Decision:** les écrans des dix ressources de lymaille et d'`app-settings` forment une seule Solution livrée ensemble, plutôt qu'un
      amorçage suivi du reste.
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-3 - Une liste par type, badge d'espace**
    - **Decision:** voir `FR-1`.
    - **Alternatives:** deux onglets « Mes… » et « Catalogue » (écarté : double le nombre de vues et de contrôles).
    - **Source:** décision du développeur principal, 2026-11-13

- **DEC-4 - Référence de secret, jamais de clé**
    - **Decision:** voir `FR-4`.
    - **Source:** miryad-core `lymaille#DEC-16`

- **DEC-5 - On s'arrime au tooling de miryad-core ; les types des définitions viennent d'un export amont, en attendant des miroirs gardés**
    - **Decision:** l'application produit l'IR de ses ressources (`resource_ir`, fichier `ir.json` par un binaire de `app`) ; les écrans de
      liste et les types des colonnes plates en sont dérivés. Les formes des colonnes de définition (`AgentDefinition`, `ModelProfile`,
      `Toolset`, fournisseur) devraient venir d'un export ts-rs de lymaille et de miryad-core, comme les événements de session : cet export
      est une **demande amont** (`OI-3`, miryad-core#72). Tant qu'il n'existe pas, vanyline écrit des miroirs Rust qui dérivent `TS`
      (`app/src/definitions/`), gardés par un test de contrat qui compare le JSON d'une valeur construite par la crate à celui du miroir, dans
      les deux sens ; les miroirs sont retirés quand l'export amont existe.
    - **Alternatives:** miroirs sans garde (écarté : dérive silencieuse) ; attendre l'export avant d'écrire les écrans (écarté : le frontend
      n'attend pas) ; JSON brut sans types fins (écarté : formulaires peu guidés).
    - **Consequences:** voir `OI-1` pour l'écart de bibliothèque de composants du générateur, `OI-3` pour la demande amont.
    - **Source:** décision du développeur principal, 2026-11-13 (« miryad-core fournit le tooling, on doit arrimer »), précisée par la vérification de `CON-5`

## Réalisation SpecDD

- `app/src/` : binaire de production de l'IR (`ir`) listant les ressources montées ; amendement de `main.sdd`.
- `app/src/definitions/` : miroirs ts-rs des définitions et des lignes de ressource, exportés vers
  `packages/protocol/src/generated/definition-domain.ts` (remplace le `config-domain.ts` de l'ancien modèle).
- `packages/ui/src/config/` : un écran par ressource et la coque `ConfigShell` (les écrans retirés par D3 sont remplacés) ; spec par
  composant.
- `frontend/src/api/` : le client de ressources qui remplace `httpConfigRepo.ts` ; `SettingsView` amendée.
- Le tableau des groupes de navigation de `/settings` : Compte, Configuration (utilisateur), Administration (administrateur).

## Acceptation et vérification

- **AC-1:** Une liste d'agents affiche les agents privés et ceux du catalogue avec leur badge ; un utilisateur ordinaire ne peut pas modifier
  une ligne du catalogue ; « Dupliquer » crée une copie privée sans préfixe `G_`.
- **AC-2:** Un utilisateur sans le groupe `admin` ne voit pas les écrans de fournisseurs, profils, serveurs MCP et types de contexte ; un 403
  du serveur s'affiche comme un manque de droits.
- **AC-3:** Créer puis modifier un profil, un agent et un toolset passe par le client de ressources ; un champ vidé est envoyé vidé ; la ligne
  courante est conservée pour tout champ non édité ; un test de contrat accepte des définitions réelles de lymaille.
- **AC-4:** L'écran d'un fournisseur n'a aucun champ de clé d'API et n'affiche aucune valeur de secret ; la référence est saisie et relue.
- **AC-5:** Un compte sans profil utilisable voit l'assistant indisponible et le parcours d'amorçage adapté à son rôle ; l'administrateur règle
  `default_profile`.
- **AC-6:** Après un tour de l'agent de configuration qui modifie une ressource, la liste ouverte est rechargée.
- **AC-7:** Les écrans sont utilisables à 360 px et ne font pas défiler la page.

## Open Issues

- **OI-1 - Bibliothèque de composants du générateur.** Le `scaffold` du template `miryad` génère des écrans List/Form/View en **shadcn-vue** ;
  le frontend de vanyline utilise **Nuxt UI** et Reka UI. Brancher le générateur tel quel mélangerait deux bibliothèques. **TBD :** adapter
  ou paramétrer ses templates pour Nuxt UI (côté `miryad`), ou n'en retenir que l'IR et les types et écrire les listes à la main.
- **OI-2 - Essais de connexion.** Les écrans actuels testent un fournisseur (liste des modèles) et un serveur MCP (liste des outils) par des
  routes propres de l'ancien backend, retirées. **TBD :** une route de `app` qui fait cet essai sous les droits de l'utilisateur, ou s'en
  passer (l'échec se voit à l'usage).
- **OI-3 - Export TypeScript des définitions.** Les types de lymaille (`AgentDefinition`, `ModelProfile`, `Toolset`, fournisseur), les lignes
  des ressources de miryad-core et les corps de requête de session ne sont pas exportés vers TypeScript (`CON-5`). **Décision :** vanyline écrit des
  miroirs Rust dérivant `TS` (`app/src/definitions/`) et la demande amont est ouverte, [miryad-core#72](https://github.com/sebt3/miryad-core/issues/72)
  (feature `ts-rs` étendue de miryad-core et de lymaille, sur le modèle de `export_typescript` des événements). À préciser en spec : lesquels
  de leurs champs les formulaires exposent en première version.
- **OI-4 - Contextes et types de contexte.** Ces objets n'ont jamais eu d'écran. **TBD :** ce qu'un utilisateur ordinaire en fait (créer un
  contexte privé, lier un rôle MCP) et ce qui reste réservé à l'administrateur, à partir de `lymaille#DEC-20`.

## Références

- `dsd/app.md` : `BR-3`, `FR-1`, `CON-3`, `OI-2`, `OI-7`.
- `dsd/app/sessions.md` : `FR-5`, `DEC-6` ; `app/src/sessions/catalog.sdd` (agents, types et contextes publiés).
- `dsd/app/widgets.md` : `OI-3` (`script_grants` des types de contexte).
- miryad-core 0.3.1 : `dsd/miryad-core/lymaille.md` (`DEC-7`, `DEC-9`, `DEC-14`, `DEC-16` à `DEC-20`), `dsd/miryad-core/frontend.md` (IR).
- Dépôt `miryad` (template) : `scaffold/` et `docs/frontend.md`.
- `packages/ui/src/config/`, `frontend/src/components/SettingsView.sdd`, `frontend/src/api/httpConfigRepo.sdd`.
