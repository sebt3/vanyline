# Solution Design: Modèle de menace de l'agent de code

## Résumé

Cette Solution dit ce qui borne les effets de l'agent de code, et ce qu'elle ne borne pas. Les toolsets accordés bornent l'intention du LLM, jamais les effets : un `build.rs` ou un script d'installation exécute du code arbitraire (`sandbox#BR-9`). Les effets se bornent donc au niveau du pod, par le réseau, l'identité, le conteneur et les ressources. Elle distingue deux garanties contractuelles, des bornes posées par défaut, et des risques résiduels nommés.

## Problème et objectif

Une sandbox exécute le code d'un projet et celui que l'agent écrit ou installe. Une approbation humaine par commande ne protège pas mieux contre la malveillance, et la philosophie du projet est de poser les limites avant la session (`app#DEC-3`). Il faut dire noir sur blanc ce que le pod empêche, ce qu'il n'empêche pas, et lesquels de ces choix sont réglables par l'Owner (`sandbox#OI-1`).

## Périmètre et non-objectifs

Couvre : la liste des interdits garantis, les bornes par défaut (réseau, identité, conteneur, ressources, noyau), les risques résiduels.

Hors périmètre : l'authentification de la sandbox (`sandbox/access`), le contenu du pod (`sandbox/runtime`), les politiques réseau elles-mêmes (`workspaces/network`), la protection des branches git, qui relève de la forge (`app#DEC-4`).

## Contexte applicable du domaine

- `sandbox#BR-9`, `sandbox#DEC-3` : les effets sont bornés au niveau du pod.
- `sandbox#BR-1`, `sandbox#BR-2` : un seul mécanisme d'authentification, et un ServiceAccount d'Owner sans droit RBAC.
- `sandbox#BR-6` : le confinement des chemins est une garde d'ergonomie ; la frontière de sécurité réelle est le pod.
- `sandbox#DEC-2` : la clé SSH de l'Owner est dans le pod et `git push` n'est pas restreint.
- `sandbox#OI-1`, `sandbox#OI-2`, `sandbox#OI-3` : ouverts par le domaine, traités ici ou renvoyés.
- `workspaces#BR-1` : un Owner = un namespace ; `workspaces#BR-6` : le réseau sortant est l'union des règles des trois niveaux.

## Acteurs

- **Agent de code :** un LLM, et tout ce qu'il lance : scripts, `build.rs`, scripts d'installation de dépendances.
- **Dépendance tierce :** code téléchargé et exécuté pendant un build.
- **Owner :** propriétaire de la sandbox, qui déclare ses règles réseau et ses ressources.
- **Autre Owner :** tenant voisin, à ne jamais atteindre.
- **Administrateur du cluster :** fournit CNI, RuntimeClass et quotas.

## État actuel et état cible

- **Actuel (Observed in: `controller/src/sandbox.rs`, `controller/src/project.rs`) :** le pod pose `runAsNonRoot`, l'UID et `fsGroup`, sans réglage de capabilities, d'escalade ni de seccomp ; il monte le jeton du ServiceAccount de l'Owner (sans RBAC) ; sans règle `egress` aux trois niveaux aucune politique de sortie n'est produite (`workspaces/network#FR-4`) ; `spec.resources` est vide par défaut, sans quota ; aucune RuntimeClass.
- **Cible :** les décisions `DEC-2` à `DEC-4`. Aucune spec du controller n'est encore amendée : voir Réalisation SpecDD.

## Hypothèses, contraintes et dépendances

- **CON-1:** Un script lancé dans la sandbox s'exécute avec les droits de l'utilisateur du pod et peut tout ce que ce pod peut.
    - **Source:** `sandbox#BR-9`

- **CON-2:** L'accès à Internet est nécessaire au travail normal (registres de paquets, forge) : ce qui est lu dans le pod peut donc en sortir.
    - **Source:** `sandbox#DEC-2` ; décision du développeur, 2026-11-07

- **CON-3:** L'isolation réseau dépend d'un CNI qui applique les NetworkPolicy, y compris `ipBlock` avec `except`.
    - **Source:** `workspaces/network#FR-3`

## Exigences fonctionnelles

- **FR-1 - MUST:** Un script exécuté dans une sandbox ne peut lire ni les volumes, ni les jetons, ni les sandboxes d'un autre Owner, réseau compris. L'isolation repose sur un namespace par Owner, sur la politique d'entrée de chaque sandbox, et sur l'audience dédiée du jeton d'Owner.
    - **Source:** décision du développeur, 2026-11-07 ; `workspaces#BR-1` ; `workspaces/network#FR-6` ; `sandbox#BR-1`
    - **Verified by:** `AC-1`

- **FR-2 - MUST:** Le conteneur de la sandbox n'obtient ni escalade de privilèges ni capabilities : `allowPrivilegeEscalation` est faux, toutes les capabilities sont retirées, le profil seccomp est `RuntimeDefault`, en plus de `runAsNonRoot` déjà posé.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-2`

- **FR-3 - MUST:** Quand l'union des règles `egress` des trois niveaux est vide, le controller produit une politique de sortie qui autorise le DNS et l'Internet public, et refuse les plages privées du cluster, l'API Kubernetes et les métadonnées du nœud. Les règles déclarées s'ajoutent à ce défaut.
    - **Source:** décision du développeur, 2026-11-07 ; modifie `workspaces/network#FR-4`
    - **Verified by:** `AC-3`

- **FR-4 - MUST:** Le pod ne monte aucun jeton Kubernetes (`automountServiceAccountToken` faux) : la clé publique de vérification des jetons d'Owner est fournie au serveur de la sandbox par un volume.
    - **Source:** décision du développeur, 2026-11-07 ; `sandbox#BR-2`
    - **Verified by:** `AC-4`

- **FR-5 - MUST:** Quand `spec.resources` d'une Sandbox est vide, le controller applique une demande et une limite par défaut, réglables par l'Application ; une `ResourceQuota` borne le namespace de chaque Owner.
    - **Source:** décision du développeur, 2026-11-07
    - **Verified by:** `AC-5`

- **FR-6 - MAY:** Une Sandbox ou une Application peut désigner une `RuntimeClass` (gVisor, Kata) ; rien ne l'exige quand le cluster n'en offre pas.
    - **Source:** décision du développeur, 2026-11-07 ; `sandbox#OI-2`
    - **Verified by:** `AC-6`

## Décisions

- **DEC-1 - Deux interdits garantis, le reste en bornes par défaut**
    - **Context:** `sandbox#OI-1` demande la liste de ce que l'agent ne doit jamais pouvoir faire.
    - **Decision:** sont garantis `FR-1` (ne jamais lire un autre Owner) et `FR-2` (ne pas s'échapper du conteneur). L'accès à l'API Kubernetes et au cluster, et l'épuisement du nœud, ne sont pas des garanties : ce sont des bornes posées par défaut (`DEC-2` à `DEC-4`), que l'Owner peut relâcher en déclarant ses propres règles et ressources. Ce qui reste permis est nommé en risques résiduels.
    - **Source:** décision du développeur, 2026-11-07

- **DEC-2 - La sortie par défaut est « Internet oui, cluster non »**
    - **Context:** sans règle, aucune politique n'est produite et le pod voit tout le cluster.
    - **Decision:** `FR-3`.
    - **Alternatives:** tout refuser par défaut (écartée : rien ne fonctionne sans configuration, contraire à « install = prêt à l'emploi ») ; rester libre (écartée : un script sonde le cluster).
    - **Source:** décision du développeur, 2026-11-07

- **DEC-3 - Le pod ne porte aucun secret Kubernetes et son conteneur est durci**
    - **Decision:** `FR-2` et `FR-4`.
    - **Consequences:** le serveur de la sandbox ne lit plus le JWKS de l'apiserver avec son jeton : la source des clés de vérification est à fixer (`OI-1`).
    - **Source:** décision du développeur, 2026-11-07

- **DEC-4 - Des limites par défaut et une RuntimeClass optionnelle**
    - **Decision:** `FR-5` et `FR-6`. L'isolation noyau est un renfort que l'administrateur du cluster active, jamais une condition de fonctionnement.
    - **Source:** décision du développeur, 2026-11-07

## Risques résiduels

- **Exfiltration vers Internet :** tout ce que le pod lit (code du projet, secrets de dépôt, clé SSH de l'Owner) peut sortir par l'Internet public autorisé (`CON-2`). Le DNS non restreint est un canal de plus (`OI-3`).
- **Rayon d'action de la clé SSH :** l'agent peut pousser vers tous les dépôts accessibles par la clé de l'Owner (`sandbox#DEC-2`) ; la protection des branches relève de la forge.
- **Cluster et nœud sur règle de l'Owner :** un Owner qui déclare une règle vers le cluster ou qui relâche ses ressources relâche ses propres bornes (`DEC-1`).
- **Chaîne d'approvisionnement :** un paquet malveillant s'exécute avec les droits du pod pendant le build. Les bornes ci-dessus limitent ses effets, pas son exécution.
- **Gardes de chemin incomplètes :** le confinement des chemins est une garde d'ergonomie (`sandbox#BR-6`) ; ses lacunes connues (URI LSP, `edit_and_check`, `purge --workspace ""`) ne sont pas des évasions, la frontière réelle étant le pod, mais elles peuvent détruire des données de l'Owner dans son propre volume (`sandbox/maintenance#OI-10`).

## Réalisation SpecDD

Aucune spec n'est amendée à ce jour. À amender : `SpecDD /controller/src/sandbox.sdd` (contexte de sécurité du conteneur, absence de jeton monté, ressources par défaut, politique de sortie par défaut, `runtimeClassName`), `SpecDD /controller/src/owner.sdd` (`ResourceQuota` du namespace), `SpecDD /crds/src/lib.sdd` (champ `runtimeClassName`), `SpecDD /sandbox/src/auth.sdd` (source des clés de vérification sans jeton Kubernetes). Le manifeste `deploy/sandbox/sandbox-test.yaml` suit.

## Acceptation et vérification

- **AC-1:** Depuis une sandbox de l'Owner A, aucune connexion à la sandbox de l'Owner B n'aboutit, et le volume home de B n'est pas monté ni lisible.
- **AC-2:** Un binaire `setuid` lancé dans le pod n'obtient aucun privilège ; `capsh --print` ne liste aucune capability ; le profil seccomp actif est `RuntimeDefault`.
- **AC-3:** Sans règle `egress`, un `curl` vers l'API Kubernetes, vers une adresse privée du cluster et vers les métadonnées du nœud échoue, et un `curl` vers un registre public aboutit.
- **AC-4:** Le répertoire de jeton de ServiceAccount (`/var/run/secrets/kubernetes.io/serviceaccount`) est absent du pod, et un jeton d'Owner valide reste accepté.
- **AC-5:** Une Sandbox sans `spec.resources` porte une demande et une limite ; un processus qui dépasse la limite mémoire est tué sans affecter le nœud.
- **AC-6:** Une Sandbox désignant une RuntimeClass inexistante n'est pas planifiée avec un message explicite ; sans RuntimeClass désignée elle démarre normalement.

## Open Issues

- **OI-1:** **Source des clés de vérification des jetons d'Owner** sans jeton Kubernetes monté : volume de ConfigMap, volume projeté, rafraîchissement lors d'une rotation de clé. À vérifier aussi que le serveur n'a besoin du jeton nulle part ailleurs. **TBD.**
- **OI-2:** **Plages refusées par défaut** : plages privées RFC 1918, CIDR des pods et des services de la distribution, métadonnées du nœud, IPv6. Une forge ou un registre hébergé sur une adresse privée serait refusé par `FR-3` et exigerait une règle de l'Owner. **TBD.**
- **OI-3:** **DNS comme canal d'exfiltration** : la règle DNS est sans restriction de destination (`workspaces/network#FR-2`). **TBD :** accepté ou restreint au résolveur du cluster.
- **OI-4:** **Valeurs par défaut des limites et du quota** (CPU, mémoire, stockage éphémère, nombre de processus) et leur réglage par l'Application. **TBD.**
- **OI-5:** **Workflow privilégié qui ouvre le même workspace** : il lit des instructions écrites par un contexte de moindre confiance (`sandbox#OI-3`, `app#BR-3`). Renvoyé tel quel, non traité ici. **TBD.**
- **OI-6:** **Interdits non retenus comme garanties** : l'accès au cluster et l'épuisement du nœud sont des bornes par défaut (`DEC-1`). À confirmer que ce classement est voulu.

## Références

`sandbox` (`sandbox#BR-1`, `BR-2`, `BR-6`, `BR-9`, `DEC-2`, `DEC-3`, `OI-1` à `OI-3`), `app#DEC-3`, `app#DEC-4`, `app#BR-3`, `workspaces` (`BR-1`, `BR-6`), `workspaces/network` (`FR-2`, `FR-3`, `FR-4`, `FR-6`), `sandbox/maintenance#OI-10`.
