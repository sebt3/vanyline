# Solution Design: Sessions lymaille hébergées dans `app`

## Résumé

La Solution `app/sessions` décrit comment `app` héberge les sessions de conversation de lymaille par l'intégration de
miryad-core (≥ 0.3.1, features `lymaille` et `matrix`) : le trait `LymailleHost` que vanyline fournit (serveur MCP de la
sandbox, contextes de Project et de Sandbox), le catalogue des types de contexte et des agents qui font de « j'ouvre
le chat depuis cet écran » le seul sélecteur de session, le contrat des événements `Host` que vanyline type pour
l'interface, la limite de durée des sous-agents, et le transport vers le frontend (REST + SSE).

Elle réalise `app#BR-1`, `app#BR-2`, `app#DEC-1`, `app#DEC-2` et `app#FR-1`, et solde `app#OI-1` (amendements demandés à
miryad-core, livrés en 0.3.0) et `app#OI-4` (réplicas). Elle ne redéfinit pas le contrat de miryad-core, qu'elle
référence (`dsd/miryad-core/lymaille.md` du dépôt voisin `miryad-core`, `DEC-20`, `DEC-24` à `DEC-30`, `FR-11` à
`FR-15`).

## Problème et objectif

`app` porte aujourd'hui un chat maison : WebSocket (`app/src/ws/chat.rs`), type `ChatEvent`, agents système
(`app/src/system_agents.rs`), tables `vanyline_conversations`, `vanyline_messages`, `vanyline_chat_contexts`. Ce
harnais ne répond pas à la base voulue (`app#DEC-1`). Objectif : que l'utilisateur ouvre une session depuis un écran
(accueil, projet, sandbox, configuration) et que tout le reste (nature de la session, agent, modules, serveurs MCP,
limites) en découle, sans approbation en cours de session, avec le moins de code vanyline possible au-dessus de
lymaille et de miryad-core.

## Périmètre et non-objectifs

Couvert :

- le montage de l'hôte lymaille dans `app` (features, migrations, routes `/api/v1/lymaille`) ;
- l'implémentation de `LymailleHost` : `resolve_mcp` pour la sandbox, `resolve_context` pour Project et Sandbox ;
- le catalogue : types de contexte et agents par défaut (remplacent les quatre agents système) ;
- le contrat des événements `Host` (forme, taille, durabilité) et leur export TypeScript ;
- le plafond de durée des sous-agents de workflow ;
- le transport vers le frontend et le retrait du chat WebSocket à parité (`app#DEC-6`).

Hors périmètre :

- le contenu des événements `Host` (actions d'interface, tableaux éphémères) : Solutions `app/ui-actions` et
  `app/dashboards` ; cette Solution ne fixe que l'enveloppe ;
- l'écran de conversation et la liste des sessions (`app#FR-13`) : frontend ;
- l'hôte lymaille de la CLI, autonome (`cli#DEC-4`) ;
- le moteur de workflow et ses kinds : `app/workflows` ;
- le pont Matrix en usage (aucun usage à court terme, voir `CON-6`).

## Contexte applicable du domaine

- `app#BR-1` : une session s'ouvre depuis un contexte ; nature et agent par défaut en découlent.
- `app#BR-2` : limites posées par le contexte avant la session, aucune approbation en cours de session.
- `app#BR-3` : la configuration ne se modifie que par une session `Config` ; la promotion est humaine.
- `app#CON-2` : le navigateur ne voit jamais le jeton de l'Owner.
- `app#NFR-2` : événements typés, types générés pour TypeScript depuis Rust.
- `sandbox#BR-1` : seul le jeton du ServiceAccount de l'Owner ouvre la sandbox ; le droit de l'obtenir est décidé par `app`.

## Acteurs

- **Utilisateur :** ouvre une session depuis un écran ; son `subject` est la seule identité persistée par la session.
- **`app` (hôte) :** fournit `LymailleHost`, décide du lien utilisateur↔Owner, mint le jeton de l'Owner.
- **miryad-core :** stockage, sessions interactives, flux, droits, étapes de workflow `subagent`.
- **Sandbox :** serveur MCP de l'Owner, seule surface d'action sur le code.
- **Administrateur :** publie le catalogue (types de contexte, agents `G_`).

## État actuel et état cible

- **Actuel (Observed in : `app/src/ws/chat.rs`, `app/src/system_agents.rs`, `app/src/api/conversations.rs`,
  `lib/src/k8s.rs`, `app/src/db/entities/owner_links.rs`) :** chat WebSocket et `ChatEvent` ; quatre agents système
  (config, accueil, projet, sandbox) créés par `ensure_system_agents` ; `VnlK8sClient::owner_token(owner)` mint le jeton
  d'Owner (cache par UID de SA) ; `sandbox_mcp_url(owner, sandbox)` rend l'URL `/mcp` ; la table
  `vanyline_owner_links` porte le lien utilisateur↔Owner.
- **Cible :** les trois briques ci-dessus sont réutilisées derrière `LymailleHost` ; les agents système, le chat
  WebSocket et `ChatEvent` sont retirés une fois la parité atteinte (`DEC-6`).

## Hypothèses, contraintes et dépendances

- **CON-1:** `miryad-core` ≥ 0.3.1, features `lymaille` et `matrix` (hors défaut) ; `vynil-core` ≥ 0.9.1 et `lymaille` 0.2.1 en
  découlent. La version 0.3.0 ne compile pas avec `vynil-core` 0.9.1 et ne doit pas être visée.
    - **Source:** `CHANGELOG.md` de miryad-core, 0.3.1
- **CON-2:** Les migrations `miryad_lymaille_*` et celles de Matrix sont livrées par le `Migrator` de miryad-core, avec sa
  table de suivi propre (`seaql_migrations_miryad_core`) ; `app` les applique par l'appel existant à
  `miryad_core::migration::Migrator::up` et n'écrit aucune migration `vanyline_*` pour elles.
    - **Source:** `miryad-core/src/migration/mod.sdd`, amendements `matrix` et `lymaille` ; `app/src/main.rs`
- **CON-3:** Un seul processus `app` porte les sessions interactives : le hub d'événements est en mémoire et un tour actif
  n'est protégé qu'au sein du processus.
    - **Source:** `miryad-core#DEC-24` ; user instruction, 2026-11-12 (solde `app#OI-4`)
- **CON-4:** Les événements `Host` ne sont pas rejoués ; l'historique d'une session (messages, résumés d'appels d'outil)
  se relit par une route dédiée, avec `at` égal à l'instant de création de la session pour tous les éléments
  (limite amont de lymaille 0.2.x).
    - **Source:** `miryad-core#DEC-30` ; user instruction, 2026-11-12
- **CON-5:** Les sessions sont servies en REST + SSE sous `/api/v1/lymaille`, exemptées de la parité des ressources ; le
  `forward` de jeton exige un client Bearer et n'est pas utilisé par vanyline (l'hôte fournit les jetons, `DEC-2`).
    - **Source:** `miryad-core#DEC-25` ; user instruction, 2026-11-12
- **CON-6:** La feature `matrix` est activée pour que ses migrations soient appliquées avec celles de lymaille ; le pont
  n'est pas configuré ni monté (aucun usage à court terme).
    - **Source:** user instruction, 2026-11-12 ; vérification du « rien monté sans configuration » : `OI-3`

## Exigences fonctionnelles

- **FR-1 - MUST:** `app` fournit une seule implémentation de `LymailleHost` au montage de l'état miryad-core. Une panne
  de l'hôte est un refus (échec fermé).
    - **Source:** `miryad-core#DEC-27`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** `resolve_mcp` pour le rôle `sandbox` d'un contexte de Sandbox rend `Accordé{url, jeton, expiration}` avec
  l'URL `/mcp` de la sandbox et un jeton d'Owner frais, **seulement si** l'utilisateur est lié à l'Owner de la sandbox
  (`vanyline_owner_links`). Il est appelé à l'assemblage et à chaque appel d'outil : la révocation du lien prend effet à
  l'appel suivant, pas à la fin de la session. Sans lien, sandbox absente ou jeton impossible à obtenir : `Refusé`.
    - **Source:** `miryad-core#DEC-27`, `FR-14` ; `sandbox#BR-1`
    - **Verified by:** `AC-1`, `AC-2`

- **FR-3 - MUST:** `resolve_context` rend, pour les identifiants de préfixe `H_`, le `ToolsetContext` d'un Project ou d'une
  Sandbox dérivé de l'entité réelle ; il n'est jamais stocké ni écrit par REST, MCP ou une session `Config`. Un
  Project ou une Sandbox hors de portée de l'utilisateur rend `None`.
    - **Source:** `miryad-core#DEC-27` (3) ; `NOTE` §B3
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** Le jeton d'Owner n'est ni persisté, ni journalisé, ni transmis au navigateur ; sa durée de vie et son
  renouvellement sont ceux du cache de `VnlK8sClient::owner_token`, qui reste dans `app` (miryad-core ne garde aucun jeton).
    - **Source:** `app#CON-2` ; `miryad-core#DEC-27`
    - **Verified by:** `AC-2`

- **FR-5 - MUST:** Chaque écran de contexte ouvre la session par `(contexte, agent optionnel)` ; l'écran fournit le
  contexte, jamais un choix d'agent à l'utilisateur. La nature et l'agent par défaut viennent du type de contexte
  (Project et Sandbox : `DEC-6` ; accueil et configuration : `DEC-7`).
    - **Source:** `app#BR-1`, `app#FR-1` ; `miryad-core#DEC-28`
    - **Verified by:** `AC-4`

- **FR-6 - MUST:** Le catalogue livre au démarrage, de façon idempotente, les types de contexte et agents par défaut : agent
  productif (nature `Work`) pour accueil, projet et sandbox ; agent de configuration (nature `Config`) pour le contexte de
  configuration. Les agents du catalogue portent le préfixe `G_` et ne remplacent aucune définition publiée par un
  administrateur sous le même nom.
    - **Source:** `app#BR-1` ; `miryad-core#DEC-9`, `DEC-28`
    - **Verified by:** `AC-4`

- **FR-7 - MUST:** Un utilisateur non administrateur peut ouvrir une session `Config` dans un contexte sur lequel il a
  des droits ; la session n'écrit que dans son niveau (`User`) ; la promotion vers `Shared` ou le catalogue reste une
  action humaine hors session.
    - **Source:** `app#BR-3` ; `miryad-core#DEC-28`
    - **Verified by:** `AC-5`

- **FR-8 - MUST:** `app` publie ses événements d'interface par `EventHub::publish_host` sous un type `HostEvent`
  d'enveloppe `{kind, payload}`, charge ≤ 64 Kio, exporté en TypeScript avec les types de miryad-core
  (`export_typescript`, côté hôte) ; l'export est vérifié en CI comme les autres types générés.
    - **Source:** `app#NFR-2` ; `miryad-core#DEC-30`
    - **Verified by:** `AC-6`

- **FR-9 - MUST:** Un événement `Host` est une notification : tout effet qui doit survivre au rechargement de la page est
  persisté par la Solution qui le possède (tableau de bord, artefact), jamais porté par le seul flux.
    - **Source:** `CON-4` ; user instruction, 2026-11-12
    - **Verified by:** `AC-6`

- **FR-10 - MUST:** Le frontend lit l'historique d'une session par la route dédiée puis s'abonne au SSE ; il n'attend aucun
  rejeu de flux et n'affiche pas d'horodatage par message tant que `CON-4` s'applique.
    - **Source:** `miryad-core#DEC-30`, `CON-4`
    - **Verified by:** `AC-7`

- **FR-11 - MUST:** Le plafond de durée d'un sous-agent de workflow est réglé explicitement dans la configuration d'`app`,
  à une valeur couvrant un agent de code (30 à 45 minutes de compilation et de tests) ; la valeur par défaut de
  miryad-core n'est pas laissée implicite.
    - **Source:** `miryad-core#DEC-29` ; `NOTE` §B4
    - **Verified by:** `AC-8`

- **FR-12 - MUST:** Les erreurs propres à cette Solution portent un identifiant `VNL-APP-SES-NNN` ; les refus issus de
  miryad-core gardent leur `MRD-LYMAILLE-NNN`, sans réécriture.
    - **Source:** convention du projet (identifiant unique par erreur)
    - **Verified by:** `AC-1`

## Sécurité

- **Autorité.** L'hôte tranche « cet utilisateur peut-il agir sur cet Owner ? » à chaque appel d'outil, sous le principal
  relu par miryad-core. Un accord excessif échoue à l'usage, il n'élève jamais un droit (`miryad-core#DEC-20`, `DEC-27`).
- **Jeton.** Le jeton d'Owner reste dans `app` ; l'agent (Rhai) n'a que l'URL et l'appel d'outil, jamais la valeur du
  jeton. Le modèle de menace côté pod relève de `sandbox#threat-model`.
- **Séparation des rôles.** Une session `Work` ne modifie pas la configuration ; une session `Config` ne monte aucun client
  MCP et voit les liaisons de l'hôte en lecture seule (`miryad-core#DEC-27`, `DEC-28`).
- **Contenu d'agent.** Le `payload` d'un événement `Host` est une donnée : le frontend ne l'interprète jamais comme du code
  (`app#BR-5`).

## Décisions

- **DEC-1 - Une réplique pour les sessions interactives**
    - **Context:** le hub d'événements de miryad-core est en mémoire ; un tour actif n'est protégé qu'au sein du processus.
    - **Decision:** `app` est déployé en une réplique tant que les sessions interactives sont portées par miryad-core ;
      pas de verrou en base.
    - **Alternatives:** verrou et hub partagés en base (écartés : rouverts seulement avec un besoin de haute
      disponibilité, `miryad-core#DEC-24`).
    - **Consequences:** `app#OI-4` est soldé par cette limite ; elle est à porter dans la spec du déploiement
      (`Application`, `replicas`) ; un besoin de haute disponibilité (Kydah) rouvre la question côté miryad-core.
    - **Source:** user instruction, 2026-11-12

- **DEC-2 - L'hôte fournit les serveurs MCP ; ni `static` ni `forward`**
    - **Decision:** l'URL et le jeton de la sandbox viennent de `resolve_mcp`, sans ligne `mcp_servers` ni mode d'auth
      supplémentaire.
    - **Alternatives:** une ligne `mcp_servers` par sandbox (écartée : cycle de vie dynamique) ; `forward` du jeton OIDC
      de l'utilisateur (écarté : la sandbox n'accepte que le jeton d'Owner).
    - **Source:** `miryad-core#DEC-27`

- **DEC-3 - Un seul client Kubernetes et un seul cache de jeton**
    - **Decision:** `LymailleHost` réutilise `VnlK8sClient` (`owner_token`, `sandbox_mcp_url`) ; aucun second chemin de
      TokenRequest n'est créé pour les sessions.
    - **Consequences:** les relais de `app` (ticket WS, git, santé du code) et les sessions partagent le même cache ;
      `sandbox/access#OI-8` (TTL réel du jeton) conditionne la marge de renouvellement.
    - **Source:** `lib/src/k8s.rs`, `app/src/api/sandboxes.rs`

- **DEC-4 - Les événements `Host` sont des notifications, pas un état**
    - **Decision:** voir `FR-9`.
    - **Consequences:** `app/dashboards` et `app/artifacts` portent leur propre persistance ; un tableau éphémère
      reste éphémère (`app#Glossaire`) : il disparaît au rechargement, ce qui est le comportement attendu.
    - **Source:** `CON-4` ; user instruction, 2026-11-12

- **DEC-5 - Le chat WebSocket est remplacé, pas adapté**
    - **Decision:** le frontend consomme REST + SSE par un adaptateur du port `ChatTransport` de `@vanyline/ui` ; le chat
      WebSocket, `ChatEvent` et les agents système sont retirés à parité (`app#DEC-6`).
    - **Source:** user instruction, 2026-11-12

- **DEC-6 - Contextes `H_` : `app` choisit l'agent par écran, avec les seuls moyens de la stack**
    - **Context:** `miryad-core#DEC-28` : un contexte `H_` de l'hôte n'a pas de type de contexte et n'ouvre que `Work` avec
      un agent explicite. Project et Sandbox sont des `H_`. Décision du 2026-11-12 : pas d'amendement de la stack
      (miryad-core, lymaille, vynil-core), on construit avec ce qu'elle livre.
    - **Decision:** pour Project et Sandbox, `app` associe l'écran à un agent du catalogue (`G_vanyline_project`,
      `G_vanyline_sandbox`) par une table de correspondance dans `app/src/sessions/catalog.rs`, et ouvre la session avec cet
      agent explicite. L'utilisateur ne voit pas ce choix (`app#BR-1` respecté du point de vue de l'utilisateur). Accueil et
      configuration ne sont pas des `H_` : ce sont des contextes stockés du catalogue, typés (`DEC-7`).
    - **Alternatives:** faire nommer un type par `resolve_context` (amendement de miryad-core, écarté pour ne pas retarder le
      chantier ; reste une simplification possible, sans effet sur les contrats de cette Solution).
    - **Consequences:** la correspondance écran → agent n'est ni administrable ni visible par la session `Config` ; elle est
      isolée dans un seul fichier pour être remplacée sans douleur.
    - **Source:** user instruction, 2026-11-12

- **DEC-7 - Accueil et configuration sont des contextes stockés du catalogue**
    - **Decision:** deux contextes de niveau `Shared`, créés au démarrage de façon idempotente : `G_home` (type `G_home`,
      nature `Work`, agent par défaut `G_vanyline_home`) et `G_config` (type `G_config`, nature `Config`, agent par
      défaut `G_vanyline_config`). Tous deux sont ouvrables par tout utilisateur authentifié (`allowed_groups = all`).
    - **Source:** ASSUMPTION levée par l'instruction du 2026-11-12 (avancer sans aller-retour)

- **DEC-8 - Le chat actuel est supprimé, sans migration de données**
    - **Decision:** `vanyline_conversations`, `vanyline_messages` et `vanyline_chat_contexts` sont supprimées au retrait de l'ancien
      chemin en retirant leurs migrations d'origine (table propre : le schéma est reconstruit, pas migré) ; aucune donnée n'est
      conservée (aucune production à ce jour, confirmé le 2026-11-12). Le retrait est autorisé
      dès que la tranche mince (`AC-1` à `AC-4` : une session ouverte depuis un contexte, sur une sandbox, avec son
      agent) est verte ; la parité complète des fonctions de l'ancien chat n'est plus exigée, ce qui amende `app#DEC-6`.
    - **Source:** user instruction, 2026-11-12 (« on peut se permettre de casser pour reconstruire »)

- **DEC-9 - Identifiants `H_`**
    - **Decision:** `H_project_<nom>` et `H_sandbox_<owner>_<nom>` (caractères `[A-Za-z0-9_-]`). Si
      `identity::classify` de miryad-core refuse une forme, l'implémentation remonte la question par un `[?]` dans la spec
      `host.sdd` (règle de `docs/chantier-lymaille.md`) plutôt que de choisir.
    - **Source:** ASSUMPTION levée par l'instruction du 2026-11-12

- **DEC-10 - Pont Matrix : bibliothèque seule**
    - **Decision:** le module `miryad_core::matrix` est une bibliothèque (`pub mod matrix` derrière la feature, rien ne
      démarre sans appel explicite de l'hôte, vérifié dans `miryad-core/src/lib.rs`) ; vanyline l'active pour ses migrations
      et ne le monte pas.
    - **Source:** vérification du 2026-11-12 ; user instruction

- **DEC-11 - Marge du jeton et clients Bearer**
    - **Decision:** le renouvellement du jeton d'Owner suit le cache existant de `VnlK8sClient::owner_token` tel quel ; un
      jeton minté par appel d'outil est accepté (charge apiserver mesurée plus tard, pas anticipée). Les clients Bearer
      (CLI, outils) relèvent de `dsd/cli/hote.md`, hors de cette Solution.
    - **Source:** user instruction, 2026-11-12

## Réalisation SpecDD

Specs à créer ou à amender (une spec par fichier, aucune écrite à ce jour) :

- `app/src/sessions/mod.sdd` : montage (features, état miryad-core, routes `/api/v1/lymaille`) — `FR-1`, `FR-12`, `CON-1`, `CON-2`.
- `app/src/sessions/host.sdd` : `LymailleHost` — `FR-2` à `FR-4`, `DEC-2`, `DEC-3`.
- `app/src/sessions/catalog.sdd` : types de contexte et agents par défaut — `FR-5` à `FR-7`.
- `app/src/sessions/events.sdd` : `HostEvent` et export TypeScript — `FR-8`, `FR-9`, `DEC-4`.
- Amendements : `app/src/main.sdd` (montage), `app/src/config.sdd` (plafond de durée, `FR-11`), `controller/src/application.sdd`
  (`replicas`, `DEC-1`), `app/src/system_agents.sdd` (retrait à parité), `app/src/ws/chat.sdd` (retrait à parité).

## Acceptation et vérification

- **AC-1:** Avec un hôte de test, `resolve_mcp` rend `Accordé` pour un utilisateur lié et `Refusé` pour un utilisateur non lié,
  une sandbox absente ou un échec de TokenRequest ; les erreurs portent `VNL-APP-SES-*`.
- **AC-2:** Un lien utilisateur↔Owner supprimé en cours de session refuse l'appel d'outil suivant ; le jeton n'apparaît ni
  dans les journaux, ni en base, ni dans une réponse destinée au navigateur.
- **AC-3:** `resolve_context` rend le contexte d'un Project ou d'une Sandbox accessible et `None` sinon ; aucune écriture
  REST, MCP ni `Config` ne peut créer ou modifier un identifiant `H_`.
- **AC-4:** L'ouverture depuis chaque type d'écran (accueil, projet, sandbox, configuration) donne la nature et l'agent
  attendus sans que l'appelant nomme un agent ; le démarrage répété ne duplique pas le catalogue.
- **AC-5:** Un non-administrateur ouvre une session `Config` sur un contexte de son niveau et n'écrit rien au-delà.
- **AC-6:** Le type `HostEvent` est généré en TypeScript et la CI échoue s'il diverge ; un événement de plus de 64 Kio est
  refusé ; recharger la page ne perd aucun effet persistant.
- **AC-7:** Le frontend affiche une session reprise à partir de l'historique puis du SSE, sans horodatage par message.
- **AC-8:** Un step `subagent` de 40 minutes n'est pas interrompu par le plafond configuré.

## Open Issues

Aucun Open Issue bloquant. Ce qui était ouvert est tranché par `DEC-6` à `DEC-11` (écart `H_` sans type, contextes d'accueil et
de configuration, Matrix, tables du chat, jeton, nommage `H_`, clients Bearer). Reste à vérifier à l'implémentation, sans décision
à prendre :

- **OI-1 - Forme des identifiants `H_` face à `identity::classify`** (`DEC-9`) : à confirmer par un test ; en cas de refus,
  remonter la question.
- **OI-2 - TTL réel du jeton d'Owner** (`sandbox/access#OI-8`, 600 s observé contre 900 s demandé) : à relever, sans
  effet sur ces contrats.

## Références

- `dsd/app.md` : `BR-1` à `BR-3`, `CON-1`, `CON-2`, `DEC-1`, `DEC-2`, `DEC-6`, `NFR-2`, `OI-1`, `OI-4`.
- `dsd/sandbox.md` et `dsd/sandbox/access.md` : jeton d'Owner, `OI-8` (TTL).
- `dsd/cli/hote.md` : hôte lymaille autonome de la CLI.
- Dépôt voisin `miryad-core` : `dsd/miryad-core/lymaille.md` (`DEC-20`, `DEC-24` à `DEC-30`, `FR-11` à `FR-15`), `CHANGELOG.md`
  (0.3.0, 0.3.1), `NOTE-vanyline-orientation-lymaille.md`.
- `app/src/ws/chat.rs`, `app/src/system_agents.rs`, `lib/src/k8s.rs`, `app/src/db/entities/owner_links.rs`.
