# Solution Design: Accès à la sandbox

## Résumé

La Solution `sandbox/access` décrit qui peut entrer dans la sandbox et par quelle voie : le port principal, fermé par le jeton d'Owner (`sandbox#BR-1`) ; l'ouverture des WebSocket éditeur, par un ticket court-vécu à usage unique ; le port d'exploitation, sans authentification, réservé aux sondes et à la supervision (`/health`, `/metrics`) ; et la télémétrie du process (journaux, métriques Prometheus, traces OTLP optionnelles).

Elle est réalisée par `sandbox/src/auth.rs`, `sandbox/src/ws/ticket.rs`, le câblage de `sandbox/src/lib.rs`, le démarrage de `sandbox/src/main.rs` et `sandbox/src/telemetry.rs`. Ce document descend en exigences le contrat déjà écrit dans les specs `.sdd` ; il ne redéfinit pas les règles du domaine, qu'il référence. Reverse-engineering : chaque exigence cite sa spec ; contradictions et code non relu sont en Open Issues.

## Problème et objectif

Une sandbox appartient à un seul Owner et ne doit répondre qu'à lui (`sandbox#BR-1`). Le navigateur ne peut pas envoyer d'en-tête `Authorization` lors du handshake WebSocket, et il ne doit jamais détenir le jeton de l'Owner ni un JWT (`sandbox#BR-3`). Les sondes et la supervision doivent lire `/health` sans jeton, sans que cette porte soit ouverte sur le port principal (`sandbox#BR-4`). Objectif : fixer, pour chaque voie d'entrée, qui est admis, par quel mécanisme, à quel moment la preuve est consommée, et ce qui peut être journalisé. Cette Solution contribue à `sandbox#BO-1` pour la part accès : un pod isolé, utilisable par son seul Owner.

## Périmètre et non-objectifs

Couvert :

- l'authentification HTTP du port principal (garde `require_auth`), la validation du jeton et l'enveloppe des erreurs ;
- l'émission et le rachat des tickets WebSocket (`POST /ws/ticket`, garde des trois routes d'upgrade) ;
- le port d'exploitation (`/health`, `/metrics`), son bind et son câblage ;
- la télémétrie du process : subscriber `tracing`, filtre, recorder Prometheus, export OTLP optionnel, ressource OTel.

Hors périmètre :

- le contenu des routes protégées (MCP, git, santé du code, jobs) : Solutions `sandbox/mcp`, `sandbox/git`, `sandbox/code-health`, `sandbox/jobs` (section « Paysage des Solutions » de `dsd/sandbox.md`) ;
- le contenu des WebSocket `/ws/fs`, `/ws/terminal`, `/ws/lsp/*` une fois la garde franchie : Solution `sandbox/editor` ;
- la création du Service, de l'Ingress et des NetworkPolicy : domaine `workspaces` et specs du controller ; le TokenRequest relève de `app` et de `lib/src/k8s.sdd` ;
- la décision d'accorder un jeton d'Owner à un utilisateur (lien utilisateur↔Owner) : `app`, puis RBAC Kubernetes pour un opérateur ; le modèle de menace de l'agent de code, notamment ce qu'il peut faire du jeton présent dans son pod, relève de `sandbox#OI-1`.

## Contexte applicable du domaine

- `sandbox#BR-1` : jeton d'Owner unique, accès tout ou rien. Cette Solution en est la réalisation ; la règle n'est pas redéfinie ici.
- `sandbox#BR-2` : le ServiceAccount de l'Owner n'a aucun droit RBAC. Voir OI-6 sur la lecture du JWKS.
- `sandbox#BR-3` : le navigateur n'utilise que des tickets.
- `sandbox#BR-4` : `/health` et `/metrics` sont sur un port distinct, sans authentification, jamais publiés.
- `sandbox#DEC-1` : un seul mécanisme d'authentification.
- `sandbox#NFR-1` : toute erreur porte un identifiant `VNL-SBX-*`. Identifiants propres à cette Solution : `VNL-SBX-AUTH-001` à `006` et `VNL-SBX-WS-001` et `002`.

## Acteurs

- **Owner :** détenteur de l'identité `system:serviceaccount:<ns>:owner-<nom>`. Seule identité acceptée par la sandbox.
- **`app` :** vérifie le lien utilisateur↔Owner, obtient le jeton par TokenRequest, l'utilise pour `POST /ws/ticket` et relaie le ticket au navigateur (`auth.sdd`, Handles ; `vanyline.sdd`, Must).
- **Opérateur :** obtient le même jeton avec son contexte kubeconfig ; le droit de le faire est décidé par le RBAC Kubernetes (`auth.sdd`, Purpose).
- **Navigateur :** ne détient qu'un ticket et ouvre les trois routes WebSocket.
- **Controller :** pose la valeur attendue `owner_subject` à la création du pod (`auth.sdd`, Handles, renvoi à `controller/src/sandbox.sdd`).
- **Kubelet et supervision :** lisent `/health` et `/metrics` sur le port d'exploitation.

## État actuel et état cible

- **Actuel (vérifié par lecture du code, `sandbox/src/`) :** aucune occurrence de `no_auth`, `static_token`, `AccessLevel` ou `oidc_` dans le crate ; `AuthState::validate_token` existe ; les trois routes WS portent `ws_auth_middleware` ; `TicketStore` est dans `AppState` ; `main.rs` lance le port d'exploitation avant `AuthState::new` et propage son échec de bind ; `lib.rs` scrube le paramètre `ticket` du span HTTP (`scrub_ticket_uri`).
- **Cible :** le contrat de `auth.sdd`, `ws/ticket.sdd`, `lib.sdd` et `telemetry.sdd`. Les écarts restants sont listés en Open Issues.

## Hypothèses, contraintes et dépendances

- **CON-1:** L'API WebSocket du navigateur n'accepte pas d'en-tête `Authorization` ; le ticket est donc présenté en query string `?ticket=`.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Purpose
- **CON-2:** La query n'est pas percent-décodée. Le contrat n'est correct que pour des tickets alphanumériques, ce que seul ce serveur émet.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Accepts
- **CON-3:** Les tickets vivent en mémoire du process. Un redémarrage de la sandbox invalide tous les tickets en cours.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Handles (Stockage)
- **CON-4:** Les clés de vérification viennent soit du JWKS de l'apiserver (`jwks_url`, par défaut `https://kubernetes.default.svc/openid/v1/jwks`, lu avec le jeton de SA monté dans le pod, CA `jwks_ca_file`), soit d'un `jwks_file` exclusif, réservé au développement hors cluster.
    - **Source:** `sandbox/src/auth.sdd`, Handles
- **CON-5:** Le jeton est accepté seulement si `iss` égale `token_issuer` à l'identique, et si `aud` contient `token_audience` (défaut `vanyline-sandbox`).
    - **Source:** `sandbox/src/auth.sdd`, Handles
- **CON-6:** La garde WS s'exécute avant l'extracteur `WebSocketUpgrade`, pour éviter la conversion 401 vers 426 d'axum.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Handles ; renvoi à `docs/architecture.md`, section « WebSocket éditeur » (non relu)
- **CON-7:** Le port d'exploitation n'a ni Service ni Ingress. Cette règle est écrite dans `controller/src/sandbox.sdd`, non relu pour ce document (OI-7).
    - **Source:** `sandbox/src/lib.sdd`, Must

## Exigences fonctionnelles

- **FR-1 - MUST:** Chaque route du port principal listée dans `lib.sdd` (MCP, git, code-health, `POST /ws/ticket`) est protégée par `require_auth`. Aucune route non authentifiée n'est montée sur ce port.
    - **Source:** `sandbox/src/lib.sdd`, Must ; `sandbox/src/auth.sdd`, Handles
    - **Verified by:** `AC-7`
- **FR-2 - MUST:** Le jeton est lu depuis `Authorization: Bearer <jeton>` (préfixe exact, sensible à la casse). Un en-tête absent, non UTF-8 ou sans ce préfixe rend 401 `VNL-SBX-AUTH-001`, sans appel réseau.
    - **Source:** `sandbox/src/auth.sdd`, Accepts, Scenario `en_tete_absent_ou_mal_forme`
    - **Verified by:** `AC-1`
- **FR-3 - MUST:** La validation suit l'ordre fixé par `auth.sdd` : décodage du header et présence d'un `kid` ; disponibilité des clés ; recherche du `kid` ; type de clé et algorithme ; signature, `iss`, `aud`, `exp`, `nbf` ; enfin `sub == owner_subject`.
    - **Source:** `sandbox/src/auth.sdd`, Raises (ordre d'évaluation verrouillé)
    - **Verified by:** `AC-1`, `AC-3`
- **FR-4 - MUST:** Un jeton parfaitement valide dont le `sub` est celui d'un autre Owner rend 403 `VNL-SBX-AUTH-006`. Un jeton signé pour une autre audience, émis par un autre émetteur, ou expiré rend 401 `VNL-SBX-AUTH-002`.
    - **Source:** `sandbox/src/auth.sdd`, Must ; Scenarios `jeton_d_un_autre_owner_est_403`, `jeton_du_pod_audience_apiserver_refuse`
    - **Verified by:** `AC-1`
- **FR-5 - MUST:** Les algorithmes sont épinglés par le type de clé du JWKS : RSA vers RS256, EC P-256 vers ES256. Tout autre cas est refusé, y compris `none`, `HS*` et une clé fournie par le jeton.
    - **Source:** `sandbox/src/auth.sdd`, Handles (Algorithmes) ; Scenario `algorithmes_interdits`
    - **Verified by:** `AC-3`
- **FR-6 - MUST:** Si les clés sont indisponibles et qu'aucun cache n'existe, la réponse est 503 `VNL-SBX-AUTH-003` (`temporarily_unavailable`), jamais 401. Si le cache est périmé et que la recharge échoue, l'ancien cache continue de valider.
    - **Source:** `sandbox/src/auth.sdd`, Must ; Scenarios `panne_jwks_sans_cache_rend_503`, `panne_jwks_conserve_l_ancien_cache`
    - **Verified by:** `AC-2`
- **FR-7 - MUST:** Le rafraîchissement JWKS est borné : un `kid` inconnu déclenche au plus un refresh par 30 s ; une recharge en échec n'est pas retentée avant 30 s ; le cache est valide 3600 s.
    - **Source:** `sandbox/src/auth.sdd`, Handles ; Scenarios `kid_inconnu_un_refresh_puis_key_not_found`, `expiration_en_echec_plancher_de_30_s`
    - **Verified by:** `AC-2`, `AC-3`
- **FR-8 - MUST:** `POST /ws/ticket`, derrière `require_auth`, répond 200 avec `{"ticket": <32 caractères>, "expires_in_secs": 30}`. Le ticket porte le `subject` de l'Owner authentifié.
    - **Source:** `sandbox/src/lib.sdd`, Must (`handle_ws_ticket`) ; `sandbox/src/ws/ticket.sdd`, Exposes
    - **Verified by:** `AC-4`
- **FR-9 - MUST:** Un ticket est opaque, de 32 caractères `[A-Za-z0-9]` tirés indépendamment, et ne contient ni claim ni signature. Son TTL est de 30 s sur horloge monotone.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Must
    - **Verified by:** `AC-4`
- **FR-10 - MUST:** Le rachat retire l'entrée dans tous les cas, avant de tester l'expiration. Un ticket ne se rachète qu'une fois, y compris sous concurrence : parmi N tentatives simultanées, une seule réussit.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Must (Garantir l'usage unique)
    - **Verified by:** `AC-4`
- **FR-11 - MUST:** La garde des routes `/ws/fs`, `/ws/terminal` et `/ws/lsp/{toolchain}` lit seulement le paramètre `ticket` (première occurrence, clé exacte), consomme le ticket avant l'upgrade, et rend 401 sans upgrade si le ticket est refusé. Une requête qui passe la garde consomme le ticket, même si l'upgrade échoue ensuite.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Accepts et Must ; `sandbox/src/lib.sdd`, Must
    - **Verified by:** `AC-5`
- **FR-12 - MUST:** La garde ne transmet aucune identité en aval : aucune extension `AuthInfo` ou `TicketClaims` n'atteint le handler.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Handles ; Scenario `claims_valides_ne_sont_pas_injectes_en_aval`
    - **Verified by:** `AC-5`
- **FR-13 - MUST:** Une requête sans paramètre `ticket` rend 401 `VNL-SBX-WS-001`. Un ticket présent mais vide, inconnu, expiré, consommé ou mal formé rend 401 `VNL-SBX-WS-002`, avec un corps JSON identique et sans en-tête `WWW-Authenticate`.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Must et Handles (Indistinguabilité des rejets) ; Scenario `corps_401_verrouille_le_format_des_deux_identifiants`
    - **Verified by:** `AC-6`
- **FR-14 - MUST:** Le port d'exploitation sert exactement deux routes, `GET /health` (200, `{"status":"ok"}`) et `GET /metrics`, sans authentification, sans TraceLayer ni `track_metrics`. Il écoute sur `ops_listen`, par défaut `0.0.0.0:9090` (variable `OPS_LISTEN`).
    - **Source:** `sandbox/src/lib.sdd`, Must ; `sandbox/src/config.rs` (valeur par défaut)
    - **Verified by:** `AC-7`
- **FR-15 - MUST:** `/health` et `/metrics` ne répondent pas sur le port principal (404).
    - **Source:** `sandbox/src/lib.sdd`, Scenario `sante_absente_du_port_principal`
    - **Verified by:** `AC-7`
- **FR-16 - MUST:** Un échec de bind du port d'exploitation est propagé ; le démarrage s'interrompt. Un pod ne démarre pas à moitié sans son port d'exploitation.
    - **Source:** `sandbox/src/lib.sdd`, Must (`spawn_ops_server`) ; Scenario `spawn_ops_server_sur_port_occupe_rend_erreur` ; `sandbox/src/main.rs`
    - **Verified by:** `AC-7`
- **FR-17 - MUST:** `GET /metrics` répond 200 `text/plain; version=0.0.4; charset=utf-8` une fois le recorder Prometheus installé, et 503 sans corps avant.
    - **Source:** `sandbox/src/lib.sdd`, Must (`metrics_handler`) ; `sandbox/src/telemetry.sdd`, Exposes
    - **Verified by:** `AC-8`
- **FR-18 - MUST:** Le router principal alimente `http_requests_total{method,path,status}` et `http_request_duration_seconds{method,path}`, réponses d'erreur comprises, sans modifier la réponse.
    - **Source:** `sandbox/src/lib.sdd`, Must (`track_metrics`)
    - **Verified by:** `AC-8`
- **FR-19 - MUST:** Le subscriber `tracing` est installé une seule fois, composé ainsi : registre portant un filtre `EnvFilter` unique, couche `fmt`, couche OTel optionnelle. Sans `RUST_LOG` valide, le filtre de repli est `vanyline_sandbox=info,tower_http=debug`.
    - **Source:** `sandbox/src/telemetry.sdd`, Must et Handles
    - **Verified by:** `AC-8` (Scenarios `filtre_de_repli_quand_rust_log_absente`, non testés, voir OI-2)
- **FR-20 - MUST:** L'ordre de démarrage est : `telemetry::init` (recorder Prometheus puis subscriber), puis `Config::validate`, puis le bind du port d'exploitation, puis `AuthState::new`.
    - **Source:** `sandbox/src/telemetry.sdd`, Must ; `sandbox/src/main.rs`
    - **Verified by:** `AC-8`
- **FR-21 - MUST:** Si `--otel-endpoint` ou `OTEL_EXPORTER_OTLP_ENDPOINT` est renseigné, les spans sont exportés en OTLP/gRPC. Sans endpoint, aucune tentative d'export.
    - **Source:** `sandbox/src/telemetry.sdd`, Purpose et Accepts
    - **Verified by:** `AC-9`
- **FR-22 - MUST:** Tout échec OTel est absorbé : un avertissement sur stderr, le démarrage continue, `init` rend `Ok`. La ressource OTel porte seulement `service.name` et `service.version`.
    - **Source:** `sandbox/src/telemetry.sdd`, Must ; Scenarios `init_endpoint_otel_invalide_non_fatal_et_sans_provider`, `resource_otel_porte_seulement_nom_et_version_du_crate`
    - **Verified by:** `AC-9`

## Sécurité

- **Modèle.** La possession du jeton vaut identité d'Owner, et rien d'autre : pas de niveau, pas de groupe, pas de contournement (`sandbox#BR-1`, `auth.sdd`, Must not). Le jeton est vérifié par JWKS et jamais par TokenReview.
- **Ticket.** Capability d'accès aux trois routes WS, valable 30 s et à usage unique. Sa valeur passe dans l'URL : elle peut rester dans l'historique du navigateur, les proxies ou les logs d'ingress. Le TTL court et l'usage unique limitent ce risque, mais un ticket capturé et non présenté reste rejouable pendant 30 s (`ticket.sdd`, Handles ; `lib.sdd`, Must).
- **Journalisation.** Ni le jeton, ni `Authorization`, ni le ticket, ni les claims ne sont journalisés ou sérialisés. Le span HTTP est scrubé par `lib.rs`. Le `RUST_LOG` par défaut active `tower_http=debug`, d'où la nécessité du scrub.
- **Port d'exploitation.** Sans authentification par décision (`telemetry.sdd`, Must not). Sa protection repose sur le réseau : absence de Service et d'Ingress, NetworkPolicy (OI-7). `/metrics` expose les noms de routes et les codes HTTP ; le label `path` est brut (OI-10).
- **Jeton présent dans le pod.** Le jeton de SA monté dans le pod a l'audience de l'apiserver. La sandbox le refuse (`jeton_du_pod_audience_apiserver_refuse`). Ce qu'un agent de code peut faire de sa présence relève de `sandbox#OI-1`.
- **Lecture du JWKS.** Le SA de l'Owner n'a aucun droit RBAC propre. Il peut lire le JWKS grâce au ClusterRoleBinding `system:service-account-issuer-discovery`, lié au groupe `system:serviceaccounts` (`auth.sdd`, Tasks, vérification 2026-10-04). Voir OI-6.

## Conception

Flux d'entrée :

```text
Navigateur --(cookie)--> app --(Bearer jeton Owner)--> POST /ws/ticket          port principal
           <--(ticket, 30 s)--------------------------'
Navigateur --(WS ?ticket=...)--> /ws/fs | /ws/terminal | /ws/lsp/{tc}          garde ws_auth_middleware
Appelant   --(Bearer jeton Owner)--> /mcp | /git/* | /code-health/* | /ws/ticket  garde require_auth
Sondes, supervision --(HTTP sans auth)--> :ops_listen /health | /metrics        port d'exploitation
```

Ticket (`ws/ticket.rs`) : `TicketStore` est une `HashMap<String, (TicketClaims, Instant)>` derrière `Arc<Mutex<…>>`, partagée entre les clones d'`AppState`. `issue` insère une entrée de 30 s. `redeem` retire l'entrée puis rend les claims seulement si l'entrée était présente et non expirée. La garde appelle `redeem` sur le paramètre `ticket` de la query, puis laisse passer la requête sans extension.

## Décisions

- **DEC-1 - Démarrage refusé plutôt que contournement**
    - **Context:** l'ancien code proposait des modes `no_auth` et `static_token`, et l'auth doit être effective dès le démarrage.
    - **Decision:** une configuration sans matériel de vérification exploitable, ou sans `owner_subject`, fait échouer le démarrage. La vérification est faite localement par JWKS, sans TokenReview ni droit Kubernetes supplémentaire. Aucun drapeau, aucune variable d'environnement ne désactive la vérification, même pour le développement.
    - **Source:** `sandbox/src/auth.sdd`, Raises, Must not (droits Kubernetes), Handles, Scenario `aucun_contournement`
- **DEC-2 - Ticket opaque, en mémoire, sans JWT**
    - **Context:** le navigateur ne doit jamais voir le JWT ni le jeton d'Owner (`sandbox#BR-3`).
    - **Decision:** le ticket est un identifiant aléatoire, sans contenu. Son `subject` reste en mémoire serveur. Il n'est jamais persisté.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Must et Must not ; `vanyline.sdd`, Must not
- **DEC-3 - Garde WS avant l'upgrade**
    - **Context:** un 401 rendu après l'extracteur `WebSocketUpgrade` devient 426 dans axum.
    - **Decision:** la consommation du ticket a lieu dans un middleware, avant l'extracteur. Un ticket consommé ne peut pas être rejoué, même si l'upgrade échoue.
    - **Source:** `sandbox/src/ws/ticket.sdd`, Handles (Consommation avant upgrade) ; Must not (ne pas accepter de second rachat)
- **DEC-4 - Jeton non lié au pod en v1**
    - **Context:** un `boundObjectRef` invaliderait le jeton à la suppression du pod, au prix d'une demande de jeton par sandbox.
    - **Decision:** non retenu en v1. Le TTL du jeton est la seule borne de durée de vie. À reconsidérer si les jetons circulent plus que prévu.
    - **Source:** `sandbox/src/auth.sdd`, Tasks (décision du 2026-10-14)
- **DEC-5 - Port d'exploitation séparé et nu**
    - **Context:** les sondes de kubelet doivent répondre sans authentification (`lib.sdd`, Must : « Health probes depend on it » est une remarque de `main.rs`) ; cette porte ne doit pas donner accès au reste de l'API.
    - **Decision:** routeur distinct, deux routes, aucun middleware de trace ni de métriques. Son échec de bind est fatal.
    - **Source:** `sandbox/src/lib.sdd`, Must (`build_ops_app`, `spawn_ops_server`)
- **DEC-6 - `/metrics` sans authentification applicative**
    - **Context:** le port d'exploitation est sans authentification (`sandbox#BR-4`), et la sandbox n'a qu'un seul mécanisme d'authentification (`sandbox#DEC-1`). La spec ne justifie l'absence d'authentification de `/metrics` que par la frontière réseau.
    - **Decision:** pas d'authentification sur `/metrics`. La protection est réseau, et c'est une frontière documentée.
    - **Source:** `sandbox/src/telemetry.sdd`, Must not
- **DEC-7 - OTel non bloquant**
    - **Context:** un collecteur absent ou une URL malformée ne doit pas empêcher une sandbox de démarrer.
    - **Decision:** tout échec OTel est absorbé en avertissement. Les traces OTel ne sont pas garanties en cas d'arrêt sur erreur.
    - **Source:** `sandbox/src/telemetry.sdd`, Must, Handles

## Réalisation SpecDD

- `SpecDD sandbox/src/auth.sdd` : `sandbox/src/auth.rs` (FR-2 à FR-7, DEC-1, DEC-4). Tests : `sandbox/tests/auth.rs`.
- `SpecDD sandbox/src/ws/ticket.sdd` : `sandbox/src/ws/ticket.rs` (FR-8 à FR-13, DEC-2, DEC-3). Tests unitaires dans le même fichier.
- `SpecDD sandbox/src/lib.sdd` : `sandbox/src/lib.rs` (FR-1, FR-8, FR-14 à FR-18, DEC-5). Tests : `sandbox/tests/http.rs`.
- `SpecDD sandbox/src/telemetry.sdd` : `sandbox/src/telemetry.rs` (FR-17, FR-19, FR-21, FR-22, DEC-6, DEC-7). Tests : à écrire (`telemetry.sdd`, Tasks).
- `SpecDD sandbox/src/main.sdd` : `sandbox/src/main.rs` (FR-16, FR-20) ; `sandbox/src/config.sdd` : `sandbox/src/config.rs` (`ops_listen`, `otel_endpoint`, `token_audience`). Non relus pour ce document (voir Références).

## Acceptation et vérification

- **AC-1:** Un jeton valide de l'Owner atteint le handler ; un jeton d'un autre Owner rend 403 ; un jeton de pod (audience apiserver) rend 401 ; un en-tête absent ou mal formé rend 401 sans appel réseau.
    - **Source:** Scenarios `jeton_owner_valide_atteint_le_handler`, `jeton_d_un_autre_owner_est_403`, `jeton_du_pod_audience_apiserver_refuse`, `en_tete_absent_ou_mal_forme` (`sandbox/src/auth.sdd`)
- **AC-2:** Sans cache et apiserver en panne, 503 (pas 401) ; avec un cache périmé et une recharge en échec, l'ancien cache valide ; une recharge est bornée à 5 s.
    - **Source:** Scenarios `panne_jwks_sans_cache_rend_503`, `panne_jwks_conserve_l_ancien_cache`, `timeout_jwks_de_5_secondes`, `expiration_en_echec_plancher_de_30_s`
- **AC-3:** `alg: none`, HS256 avec clé publique, clé `oct`, `aud`, `iss`, `exp` et `nbf` non conformes sont refusés ; un `kid` inconnu ne provoque qu'un refresh par 30 s ; un jeton sans `kid` ne déclenche aucun appel réseau.
    - **Source:** Scenarios `algorithmes_interdits`, `audience_emetteur_expiration_non_conformes`, `kid_inconnu_un_refresh_puis_key_not_found`, `jeton_non_jwt_ou_sans_kid_sans_reseau`
- **AC-4:** Un ticket se rachète une fois avec les claims émis ; sous N rachats parallèles, un seul réussit ; un ticket expiré est retiré de la map ; `POST /ws/ticket` rend 200 avec 32 caractères et `expires_in_secs` à 30.
    - **Source:** Scenarios `ticket_rachete_rend_les_claims_emis`, `second_rachat_du_meme_ticket_echoue`, `rachats_paralleles_du_meme_ticket_un_seul_gagnant`, `ticket_expire_renvoie_aucun_et_disparait_de_la_map`, `emission_ticket_rend_ticket_rachetable_une_fois` (`sandbox/src/lib.sdd`)
- **AC-5:** Sans `ticket` ou avec `?other=1`, `?ticket` ou `?Ticket=` : 001. Un ticket inconnu ou vide : 002, sans upgrade. Un ticket valide sans en-têtes d'upgrade est consommé malgré le rejet aval. Le handler n'a aucune extension d'identité.
    - **Source:** Scenarios `requete_sans_ticket_rejetee_en_001`, `ticket_inconnu_rejete_en_002`, `parametre_ticket_vide_rejete_en_002`, `ticket_valide_consomme_malgre_upgrade_absent`, `claims_valides_ne_sont_pas_injectes_en_aval`
- **AC-6:** Les rejets inconnu, expiré et consommé sont indistinguables ; les deux corps 401 sont byte-exacts, sans `WWW-Authenticate`.
    - **Source:** Scenarios `rejets_inconnu_expire_et_consomme_sont_indistinguables`, `corps_401_verrouille_le_format_des_deux_identifiants`
- **AC-7:** `/health` répond 200 sans jeton sur le port d'exploitation ; `/health`, `/metrics` et `/.well-known/…` rendent 404 sur le port principal ; un port occupé rend une erreur de bind.
    - **Source:** Scenarios `sante_repond_sans_autorisation_sur_le_port_d_exploitation`, `sante_absente_du_port_principal`, `spawn_ops_server_sur_port_occupe_rend_erreur`, `spawn_ops_server_expose_health_et_metrics_sur_port_lie` (`sandbox/src/lib.sdd`)
- **AC-8:** `/metrics` rend 200 au format 0.0.4 après installation du recorder, et les compteurs `http_requests_total` apparaissent avec leurs labels. Avant l'installation, `render_metrics` rend `None`. Les tests de `telemetry.sdd` restent à écrire.
    - **Source:** Scenarios `metrics_rend_200_en_texte_prometheus`, `middleware_metriques_compte_requete_avec_labels` (`sandbox/src/lib.sdd`) ; scrub du span : `trace_ne_jamais_journalise_la_valeur_d_un_ticket` (`sandbox/src/lib.sdd`) ; `render_metrics_absent_avant_init_puis_present_apres` (`sandbox/src/telemetry.sdd`)
- **AC-9:** Une URL OTel malformée rend `Ok` avec un avertissement sur stderr, et `shutdown_otel` reste sans effet. Une ressource OTel ne porte que `service.name` et `service.version`.
    - **Source:** Scenarios `init_endpoint_otel_invalide_non_fatal_et_sans_provider`, `resource_otel_porte_seulement_nom_et_version_du_crate` (`sandbox/src/telemetry.sdd`, tests à écrire)

## Open Issues

- **OI-1 - Chantier « auth Owner-SA seule » encore ouvert dans `vanyline.sdd`:** la case du chantier reste `[ ]` avec sa liste (`config.sdd`, `main.sdd`, `mcp.sdd`, `controller/src/sandbox.sdd`, `lib/src/k8s.sdd`, `app/src/api/sandboxes.sdd`, RBAC de `app`, AGENTS.md et `docs/architecture.md`). Pourtant `auth.sdd` et `lib.sdd` marquent la partie crate sandbox comme faite (2026-10-08), et le code la confirme. **TBD :** ce qui reste de la liste n'est pas vérifié ici. `sandbox#OI-5` et l'état actuel de `dsd/sandbox.md` disent que l'implémentation est encore ouverte : ils sont en retard sur `auth.sdd`. Mettre à jour `vanyline.sdd` ou ces passages.
- **OI-2 - Contradiction dans `telemetry.sdd`:** le Scenario `filtre_de_repli_quand_rust_log_absente` lance le binaire « en mode `--no-auth` ». Ce mode est interdit par `auth.sdd` (Must not) et par `config.sdd`. **TBD :** réécrire le Scenario sans ce mode.
- **OI-3 - Reliquat `access` dans `ws/ticket.sdd`:** `Returns` dit que le rachat rend les claims « `subject` et `access` intacts », alors que `access` est retiré (Tasks `[x]`, `lib.sdd`). **TBD :** nettoyer le texte.
- **OI-4 - `ws/ticket.sdd` en retard sur le scrub:** `Handles` dit que `TraceLayer` logge l'URI complète, et `Tasks` laisse la fuite « DÉCISION MAINTENUE ». `lib.sdd` marque le scrub comme FAIT (2026-10-10), et `lib.rs` contient `scrub_ticket_uri`. **TBD :** aligner `ticket.sdd` sur `lib.sdd`.
- **OI-5 - Qui appelle `POST /ws/ticket` en production:** `ticket.sdd` parle du frontend, alors que le relais est fait par `app` avec le jeton de l'Owner (`ticket.sdd`, Tasks `[?]`). **TBD :** reformuler, ou garder le commentaire.
- **OI-6 - Lecture du JWKS et BR-2:** le SA de l'Owner peut lire le JWKS grâce à un binding cluster-wide de découverte, lié à tous les ServiceAccounts par défaut (`auth.sdd`, Tasks). `sandbox#BR-2` dit « aucun droit RBAC ». **TBD :** reformuler BR-2 dans `dsd/sandbox.md`, ou restreindre le binding dans `deploy/`.
- **OI-7 - Protection réseau du port d'exploitation:** l'absence de Service et d'Ingress est écrite dans `controller/src/sandbox.sdd` (non relu), et la NetworkPolicy n'est pas décrite. **TBD :** vérifier, puis écrire l'exigence réseau de cette Solution.
- **OI-8 - Durée du jeton:** `auth.sdd` cite un TTL de 900 s pour le jeton, et une vérification du 2026-10-04 observant `exp - iat = 600` ; `lib/src/k8s.sdd` demande 900 s, et l'apiserver impose un minimum de 600 s. **TBD :** confirmer la durée effective, et la politique de renouvellement côté `app`.
- **OI-9 - Ticket non purgé, RNG et percent-décodage:** un ticket émis et jamais présenté reste en mémoire indéfiniment. `rand` 0.8 n'est pas marqué cryptographique par sa crate. La query est lue sans percent-décodage (`ticket.sdd`, Tasks `[?]`, `Accepts`). **TBD :** purge (opportuniste ou périodique), source d'aléa explicite, et test du format.
- **OI-10 - Cardinalité de `path`:** le label `path` est l'URI brute, y compris pour les 404 (`lib.sdd`, Tasks `[?]`). **TBD :** gabarit de route, liste blanche, ou protection ingress assumée. Aussi (Tasks `[?]`) : ordre 401 et 405 sur le routeur fusionné, non épinglé par un test (`lib.sdd`).
- **OI-11 - Rejets du ticket non observables:** contrairement à `mcp_auth_errors_total`, les 401 `001` et `002` ne sont ni comptés ni journalisés (`ticket.sdd`, Tasks `[?]`). **TBD :** compteur, ou inobservabilité assumée.
- **OI-12 - Télémétrie, comportements non tranchés:** `RUST_LOG` invalide repli silencieux ; double `init` OTel ; `shutdown_otel` non appelé sur les chemins `?` de `main` ; course de `init_prometheus` ; `eprintln!` contre le harnais `print_stderr` ; variables `OTEL_*` non consignées ; aucun test pour `telemetry.sdd` (`telemetry.sdd`, Tasks). **TBD :** trancher chaque point.

## Références

- `dsd/sandbox.md` (`sandbox#BR-1` à `BR-4`, `DEC-1`, `NFR-1`, `BO-1`, `OI-1`, `OI-5`) ; `vanyline.sdd` (Must, Must not, Tasks).
- Code et specs : `sandbox/src/auth.sdd`, `ws/ticket.sdd`, `lib.sdd`, `telemetry.sdd`, et `auth.rs`, `ws/ticket.rs`, `lib.rs`, `telemetry.rs`, `main.rs`, `config.rs` (lecture ciblée).
- Renvois non relus : `controller/src/sandbox.sdd`, `lib/src/k8s.sdd`, `docs/architecture.md`, `sandbox/src/main.sdd`, `sandbox/src/config.sdd`.
