# Solution Design: Controller

## Résumé

La Solution `workspaces/controller` décrit le câblage transversal du binaire `vanyline-controller` : point d'entrée, configuration de démarrage, client Kubernetes partagé, lancement simultané des quatre reconcilers Owner, Project, Sandbox et Application dans un seul process, et type d'erreur commun `VNL-CTL-*` avec sa politique de requeue. Elle est réalisée par `controller/src/main.rs` et `controller/src/error.rs`. Le comportement propre à chaque CRD appartient aux Solutions sœurs (`workspaces/owners`, `workspaces/projects`, `workspaces/sandboxes`, `workspaces/network`, `workspaces/application`) et n'est pas redécrit ici.

## Problème et objectif

Les quatre reconcilers du domaine doivent tourner ensemble, dans un process, avec un client Kubernetes commun, une configuration unique et une convention d'erreurs lisible dans les logs. Cette Solution fixe ce câblage. Elle réalise la partie de `workspaces#DEC-2` (quatre CRDs, un seul controller) qui concerne le démarrage.

## Périmètre et non-objectifs

Couvert : le point d'entrée et le mode `--crds`, les seize drapeaux de configuration, la construction du client, la composition des images, le démarrage concurrent des flux, la journalisation des erreurs de flux, le type `ControllerError` et le contrat de requeue de ses variantes.

Hors périmètre : la logique de chaque reconcile, son status et ses manifestes (`workspaces/owners`, `workspaces/projects`, `workspaces/sandboxes`, `workspaces/application`) ; les politiques réseau (`workspaces/network`) ; le schéma des CRDs (`SpecDD /crds/src/lib.sdd`) ; le pod sandbox (domaine `sandbox`) ; le RBAC et le déploiement (`deploy/controller/controller.yaml`). Non-objectifs v1 : voir FR-7.

## Contexte applicable du domaine

* `workspaces#DEC-2` : quatre CRDs réconciliées en parallèle par un seul controller ; cette Solution en est la réalisation de câblage.
* `workspaces#BR-7` : le controller ne passe aucun champ de CRD à un shell ; ce document n'introduit aucune commande.

## Acteurs

* **Administrateur du cluster :** déploie le binaire, son ClusterRole et ses variables d'environnement (`deploy/controller/controller.yaml`, SpecDD /controller/src/main.sdd, Accepts).
* **Reconcilers Owner, Project, Sandbox, Application :** modules privés du binaire exposant `Context`, `build_controller`, `reconcile` et `error_policy`.

## État actuel et état cible

* **Actuel :** `controller/src/main.rs` existe, sans aucun test (SpecDD /controller/src/main.sdd, Tasks). `controller/src/error.rs` contient un module `#[cfg(test)]` dont les sept tests sont déclarés validés le 2026-10-06 (SpecDD /controller/src/error.sdd, Tasks).
* **Cible :** chaque Scenario de `main.sdd` et `error.sdd` a un test taggé par son titre ; les trois Scenarios d'intégration cluster ne sont validés que sur un cluster de test réel (SpecDD /controller/src/main.sdd, Done when).

## Hypothèses, contraintes et dépendances

- **CON-1:** Le client Kubernetes est construit une seule fois ; son échec est fatal (exit 1), sans retry ni attente.
    - **Source:** SpecDD /controller/src/main.sdd (Must not, Handles)
- **CON-2:** Les credentials sont ceux de `kube::Client::try_default` : ServiceAccount in-cluster d'abord, kubeconfig sinon.
    - **Source:** SpecDD /controller/src/main.sdd (Accepts)
- **CON-3:** Aucun mock de Kubernetes ; les Scenarios qui exigent un cluster sont taggés « intégration cluster ».
    - **Source:** SpecDD /controller/src/main.sdd (Tasks, harnais de tests)
- **CON-4:** Le harnais clippy interdit en production `unwrap`, `expect`, `panic!`, `dbg!`, `println!`, `eprintln!`.
    - **Source:** AGENTS.md, section Harnais clippy ; `tooling.sdd`
- **Dépendances :** `clap` 4, `tokio` 1, `kube` 4, `http` 1, `futures` 0.3, `tracing` 0.1 et `tracing-subscriber` 0.3, `thiserror` 2 ; crate `vanyline-crds` pour `crd_manifests` (SpecDD /controller/src/main.sdd, Depends ; SpecDD /controller/src/error.sdd, Depends).

## Exigences fonctionnelles

- **FR-1 - MUST:** `--crds` écrit sur stdout les quatre manifests CRD puis sort en 0, sans log, sans init tracing et sans client Kubernetes, y compris hors cluster.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Returns, Handles)
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Le binaire accepte exactement les seize drapeaux de SpecDD /controller/src/main.sdd (Exposes), chacun lisible par variable d'environnement, avec la priorité drapeau > variable > défaut compilé ; aucun `--version`.
    - **Source:** SpecDD /controller/src/main.sdd (Exposes, Accepts)
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** Les images sandbox et app sont composées `{repo}:{tag}` ; les huit images toolchain et LSP et `--restate-image` sont transmises sans transformation.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Exposes)
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** Un seul client Kubernetes, partagé par les quatre `Context` ; son échec produit un log `error` unique puis un exit 1.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Returns)
    - **Verified by:** `AC-4`

- **FR-5 - MUST:** Les quatre reconcilers tournent en concurrence sous un seul `tokio::join!`, observent leur CRD sur tout le cluster sans filtre, et aucun n'attend un autre au démarrage.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Must not) ; `workspaces#DEC-2`
    - **Verified by:** `AC-5`

- **FR-6 - MUST:** Une erreur de flux est journalisée en `warn` au message `<module> reconcile loop error` avec le champ `error`, et le flux continue ; un reconcileur en échec n'arrête ni les autres ni le process.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Handles)
    - **Verified by:** `AC-6`

- **FR-7 - MUST NOT:** Le binaire n'installe ni gestionnaire de signal, ni arrêt gracieux, ni leader election, ni endpoint de santé, ni métriques, ni port HTTP, ni redémarrage d'un reconcileur terminé.
    - **Source:** SpecDD /controller/src/main.sdd (Must not, Handles)
    - **Verified by:** `AC-7`

- **FR-8 - MUST:** Toute erreur de reconcile est une `ControllerError` dont le `Display` commence par un marqueur `VNL-CTL-<NNN>` unique suivi de `: `, aux textes de SpecDD /controller/src/error.sdd (Exposes).
    - **Source:** SpecDD /controller/src/error.sdd (Must, Exposes) ; AGENTS.md, section Git (messages d'erreur)
    - **Verified by:** `AC-8`

- **FR-9 - MUST:** Politique de requeue : owner et application, 30 s pour toute erreur ; project, 10 s en `info` pour `PurgePending`, 30 s en `warn` sinon ; sandbox, 10 s en `info` pour `WorktreeRemovalPending`, 15 s en `info` pour `ProjectNotReady`, 30 s en `warn` sinon.
    - **Source:** SpecDD /controller/src/error.sdd (Contrat de consommation)
    - **Verified by:** `AC-9`

- **FR-10 - MUST NOT:** Une erreur de reconcile n'arrête jamais un reconcileur (ni `Action::teardown`, ni `Action::await_change` sur erreur) et n'entraîne jamais un exit ni une panique.
    - **Source:** SpecDD /controller/src/error.sdd (Must, Must not) ; SpecDD /controller/src/main.sdd (Raises)
    - **Verified by:** `AC-10`

- **FR-11 - MUST:** Les logs passent par `tracing` après l'init ; seul `print!` du mode `--crds` écrit sur stdout hors logs ; `ControllerError` n'émet ni log ni événement Kubernetes.
    - **Source:** SpecDD /controller/src/main.sdd (Must not) ; SpecDD /controller/src/error.sdd (Must not)
    - **Verified by:** `AC-11`

## Conception

Séquence de `main` (ordre strict, SpecDD /controller/src/main.sdd, Structure) :

```text
parse Cli (clap : drapeau > env > défaut)
  `-- --crds ? -> print! des manifests, return (code 0)
init tracing (try_init, résultat ignoré) ; log info de démarrage
client = kube::Client::try_default()        -- échec : log error, exit 1
images : sandbox et app = repo:tag ; toolchain/LSP/restate verbatim
Context x4 (même client, Arc)
tokio::join!( owner.run, project.run, sandbox.run, application.run )
  chaque flux : .for_each(warn "<module> reconcile loop error")
```

Câblage des `Context` (SpecDD /controller/src/main.sdd, Must) : `owner` reçoit `client` ; `project` reçoit `client` et `sandbox_image` ; `application` reçoit `client`, `default_image` (app composée) et `restate_image` ; `sandbox` reçoit `client`, `default_image` (sandbox composée) et les huit images toolchain et LSP. Chaque module expose `build_controller(client)` (watcher `Api::all`) puis `run(reconcile, error_policy, ctx)`.

Typage d'erreur (SpecDD /controller/src/error.sdd) : `ControllerError` est un `enum` privé, dérivé de `Debug` et `thiserror::Error` seulement. Seule `Kube` porte `#[from]` et une `source()`. `Finalizer(String)` stringifie sans chaîner. Marqueurs : `001` Kube, `002` OwnerNotReady, `003` PurgePending, `004` Finalizer, `005` ProjectNotReady, `006` WorktreeRemovalPending, `007` InvalidNamespaceName, `008` NamespaceCollision, `009` NamespaceTerminating, `010` NamespaceNotManaged, `011` SandboxIssuerUnavailable, `012` SandboxNamespaceMissing.

`PurgePending` et `WorktreeRemovalPending` sont des signaux de contrôle, pas des anomalies : elles font échouer volontairement le cleanup pour que le finalizer kube-rs ne se retire pas, et sont journalisées en `info` (SpecDD /controller/src/error.sdd, Handles).

## Décisions

- **DEC-1 - Un process, quatre reconcilers**
    - **Context:** Les quatre CRDs forment un système réconcilié unique et partagent client et images.
    - **Decision:** Un binaire lance les quatre flux en concurrence via `tokio::join!`.
    - **Consequences:** Un reconcileur qui meurt silencieusement n'est pas redémarré (OI-5).
    - **Source:** `workspaces#DEC-2` ; SpecDD /controller/src/main.sdd (Purpose, Must)

- **DEC-2 - Un client Kubernetes partagé**
    - **Context:** Les quatre reconcilers appellent la même apiserver.
    - **Decision:** Un `Client` construit une fois, cloné dans chaque `Context` derrière un `Arc`.
    - **Consequences:** L'échec du client est global et fatal ; aucune reconnexion par reconcileur.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Handles)

- **DEC-3 - Watch cluster-wide, sans filtre de namespace**
    - **Context:** Les CRDs vivent dans le namespace d'application et dans les namespaces d'Owner.
    - **Decision:** Chaque reconcileur observe tout le cluster ; le ClusterRole de `deploy/controller/controller.yaml` est cohérent avec ce choix.
    - **Consequences:** Le périmètre de droits du controller est cluster-wide ; `workspaces/owners` (FR-3) limite les effets au namespace étiqueté.
    - **Source:** SpecDD /controller/src/main.sdd (Must, Must not, Accepts)

- **DEC-4 - Erreurs typées, marqueur stable, transport seul**
    - **Context:** Les marqueurs sont lus dans les logs et recherchés par grep.
    - **Decision:** Une enum unique à marqueurs `VNL-CTL-NNN`, jamais un code d'arrêt du binaire.
    - **Consequences:** Renommer ou renuméroter un marqueur exige une décision de spec.
    - **Source:** SpecDD /controller/src/error.sdd (Purpose, Must, Must not) ; AGENTS.md, section Git (messages d'erreur)

- **DEC-5 - Politique de requeue par reconcileur**
    - **Context:** Les variantes de contrôle appellent un délai plus court que les erreurs réelles.
    - **Decision:** Chaque `error_policy` distingue les variantes qui la concernent ; les autres retombent sur 30 s en `warn`.
    - **Consequences:** Owner et application ne distinguent aucune variante (OI-4).
    - **Source:** SpecDD /controller/src/error.sdd (Contrat de consommation, Handles)

## Réalisation SpecDD

- `SpecDD /controller/src/main.sdd` : `controller/src/main.rs` (FR-1 à FR-7, FR-11 ; DEC-1 à DEC-3).
- `SpecDD /controller/src/error.sdd` : `controller/src/error.rs` (FR-8 à FR-11 ; DEC-4, DEC-5).
- Contrats consommés, réalisés par les Solutions sœurs : `error_policy` de `SpecDD /controller/src/owner.sdd`, `project.sdd`, `sandbox.sdd`, `application.sdd` (FR-9).

## Acceptation et vérification

- **AC-1:** `--crds` sort en 0 avec quatre `CustomResourceDefinition` sur stdout et stderr vide (Scenarios `crds_imprime_quatre_manifests_et_sort`, `crds_sort_avant_tracing_et_client_kube`).
- **AC-2:** Les seize drapeaux, leurs défauts et la priorité env/flag sont vérifiés (Scenarios `cli_*`, dont `cli_aide_liste_les_seize_drapeaux` et `cli_version_absente_rejelee`).
- **AC-3:** L'image sandbox composée atteint le pod (Scenario `image_sandbox_composee_propagee_au_pod`, intégration cluster).
- **AC-4:** Cluster injoignable : un log `error`, exit 1, sans retry (Scenario `client_kube_injoignable_exit_1`).
- **AC-5:** Dans un cluster de test, les quatre reconcilers tournent et le process ne sort pas (Scenario `demarrage_lance_les_quatre_reconcilers_cluster`, intégration cluster).
- **AC-6:** Un reconcileur en erreur durable n'arrête pas les autres (Scenario `reconcile_en_erreur_ne_tue_pas_le_processus`, intégration cluster).
- **AC-7:** Aucune route HTTP ni gestionnaire de signal dans `main.rs` ; vérifié par revue, aucun Scenario dédié (OI-5).
- **AC-8:** Les douze marqueurs sont distincts et leur `Display` est exact ; seule `Kube` rend une `source()` (Scenarios `display_exhaustif_des_marqueurs_vnl_ctl`, `conversion_from_kube_error_et_source_chainee`, `source_absente_sur_les_cinq_autres_variantes`).
- **AC-9:** Délais et niveaux de log des quatre politiques verrouillés (Scenarios `error_policy_*`).
- **AC-10:** Aucune action teardown ou await_change sur erreur (Scenario `aucune_erreur_ne_stoppe_le_reconcile`).
- **AC-11:** Aucun `println!`, `eprintln!` ou `dbg!` hors `print!` de `--crds` (harnais clippy, voir OI-3).

## Open Issues

- **OI-1 - État des specs sœurs contradictoire:** `main.sdd` (Purpose, Depends, Tasks) et `error.sdd` (Purpose) disent les specs owner, project, sandbox et application « à écrire » ou « absentes », alors qu'elles existent dans `controller/src/`. **TBD :** confirmer leur statut et corriger les deux specs.
- **OI-2 - Compteurs obsolètes dans `error.sdd`:** l'Exposes annonce « 10 variantes », la Tasks « six marqueurs étendue à dix » et le Done when « six rendus », alors que l'Exposes et le Scenario `display_exhaustif_des_marqueurs_vnl_ctl` en listent douze ; les Scenarios `aucune_erreur_ne_stoppe_le_reconcile` (24 appels) et `error_policy_owner_et_application_toujours_trente_secondes` suivent le même décompte. Le Scenario `source_absente_sur_les_cinq_autres_variantes` ne couvre que cinq variantes sur onze. **TBD :** corriger les compteurs.
- **OI-3 - `print!` de `--crds` face au harnais:** `main.sdd` (Tasks, `[?]`) : `print_stdout` est `deny` sans `#[allow]` posé. **TBD :** `#[allow]` motivé ou exemption confirmée.
- **OI-4 - Politiques owner et application sans distinction:** `error.sdd` (Tasks, `[?]`) : 30 s uniforme en v1 ou distinction de `OwnerNotReady` ? **TBD.**
- **OI-5 - Supervision et signaux:** `main.sdd` (Handles, Tasks `[?]`) : SIGTERM tue sans grace period, et un flux terminé n'est pas détecté. Accepté en v1 (liveness par K8s) ou à contracter ? **TBD.** Lié à FR-7.
- **OI-6 - Chaînage du finalizer face à la discrimination:** `error.sdd` Handles dit que `map_err` vers `Finalizer` noie les variantes de contrôle, alors que Must exige de les garder discriminées ; comportement de kube-runtime en Cleanup à vérifier (`error.sdd`, Tasks `[?]`). **TBD.**
- **OI-7 - Tags par défaut sandbox et app sans `v`:** `main.sdd` (Tasks `[?]`) : `SANDBOX_IMAGE_TAG` et `APP_IMAGE_TAG` ont la version nue, les toolchains `v<version>`, et `release.yml` publie le tag préfixé. Le doc comment de `--sandbox-image-tag` contredit le code. **TBD :** une des trois voies de la spec.
- **OI-8 - Restate épinglé:** `main.sdd` fixe `1.7.13` (décision du 2026-10-14) ; code et tests restent à aligner (Tasks). Écart spec/code. **TBD :** état du code.
- **OI-9 - Défauts de démarrage non tranchés:** `main.sdd` (Tasks `[?]`) : niveau de log sans `RUST_LOG` (le filtre par défaut masquerait l'`info` de démarrage), valeurs d'env vides (`SANDBOX_IMAGE_TAG=""` donne une image invalide), absence de `--version`. **TBD.**
- **OI-10 - Points portés par les Solutions sœurs:** cleanup best-effort et données laissées sur le PVC (`error.sdd`, Tasks `[?]`), recréation d'un Job sandbox existant (`sandbox.sdd`, `[?]`), stratégie de cleanup Project/Sandbox (`project.sdd`, `sandbox.sdd`). **TBD**, renvoyés à `workspaces/projects` et `workspaces/sandboxes`.
- **OI-11 - Référence de bump dans `owners.md`:** `owners.md` (OI-2) cite `workspaces#OI-2` pour le déclenchement du bump ; `workspaces#OI-2` porte sur le watch et la resynchronisation. **TBD :** lien à confirmer, hors de cette Solution.

## Références

- `workspaces#DEC-2`, `workspaces#BR-7` ; SpecDD `/controller/src/main.sdd`, `/controller/src/error.sdd` ; sœurs `owner.sdd`, `project.sdd`, `sandbox.sdd`, `application.sdd`.
- AGENTS.md, sections Harnais clippy et Git ; `tooling.sdd` ; `deploy/controller/controller.yaml` et `.github/workflows/release.yml`, cités par les specs, non relus pour ce document.
