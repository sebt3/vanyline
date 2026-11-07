# Solution Design: Jobs de la sandbox

## Résumé

La Solution `sandbox/jobs` fournit l'API `/jobs` du port principal de la sandbox : démarrer un script shell de longue durée dans le pod, l'interroger, en lire les logs par fenêtres, l'annuler. Elle sert les nœuds `shell` et `gate` du moteur de workflow d'`app`, qui ne peuvent pas attendre un appel synchrone. Elle est réalisée par `sandbox/src/jobs.rs` selon `SpecDD /sandbox/src/jobs.sdd`. Le registre est en mémoire : un redémarrage du pod fait échouer le nœud appelant. Document reverse : il décrit ce que la spec et le code contractent ; les points non tranchés sont en `TBD`.

## Problème et objectif

- Un nœud shell peut durer de plusieurs minutes à plusieurs heures, ce qu'un appel bloquant dans un step Restate ne supporte pas (Source: SpecDD /sandbox/src/jobs.sdd, Purpose).
- Objectif : démarrer (idempotent), interroger, lire les logs, annuler, pour que tout rejeu du step soit sûr (Source: SpecDD /sandbox/src/jobs.sdd, Purpose), en réalisant `sandbox#FR-4`.

## Périmètre et non-objectifs

Couvert : les routes `/jobs*`, le registre, l'exécution, les plafonds, les logs, le callback optionnel vers Restate.
Hors périmètre : les nœuds `shell` et `gate` d'`app` (`SpecDD /app/src/workflow/steps/shell.sdd`, `gate.sdd`, consommateurs non décrits ici) ; `execute_command` (`SpecDD /tools/src/command.sdd`) ; le modèle de menace de l'agent de code (`sandbox#OI-1`) ; la santé du code (un job n'alimente ni ne déclenche `/code-health`, SpecDD /sandbox/src/jobs.sdd, Handles).

## Contexte applicable du domaine

- `sandbox#BR-1` : accès tout ou rien par le jeton de l'Owner, appliqué à toutes les routes (FR-13).
- `sandbox#BR-6` : confinement de tout chemin sous la racine du workspace, appliqué au `cwd` (NFR-4).
- `sandbox#BR-9` et `sandbox#DEC-3` : les effets d'un script sont bornés au pod, pas par approbation ; un script de `/jobs` relève de cette règle.
- `sandbox#DEC-2` : la clé SSH de l'Owner est présente dans le pod ; un job peut tirer et pousser ses dépôts.
- `sandbox#BR-7` : un job qui modifie le worktree est détecté par l'empreinte de `SpecDD /sandbox/src/health.sdd`, sans lien direct avec `/jobs`.
- `sandbox#NFR-1` : identifiants `VNL-SBX-*` (voir OI-6). `app#DEC-3` : limites posées par le contexte, effets bornés au pod.

## Acteurs

- **Worker (nœud `shell` ou `gate` d'`app`) :** démarre le job, l'interroge, lit ses logs, peut l'annuler (SpecDD /sandbox/src/jobs.sdd, Purpose).
- **Restate (ingress) :** destination unique d'un callback, si configurée (SpecDD /sandbox/src/jobs.sdd, Handles).
- **Controller :** pose `restate_ingress_url` dans le pod quand l'Application porte `spec.restate` (SpecDD /sandbox/src/jobs.sdd, Tasks, non coché).

## État actuel et état cible

- **Actuel :** routes (`sandbox/src/lib.rs`, lignes 251-257), registre, callback et `restate_ingress_url` (`sandbox/src/config.rs`) existent (Observed in: `sandbox/src/jobs.rs`).
- **Tests :** `sandbox/tests/jobs.rs`, 14 Scenario taggés et 4 tests d'appoint (Tasks de `jobs.sdd`, FAIT 2026-10-10).
- **Non livré :** le callback utilisable par `app`. Son prérequis est un kind de step durable dans miryad-core (Tasks, sebt3/miryad-core#26). D'ici là le worker interroge `GET /jobs/{job_id}`.

## Hypothèses, contraintes et dépendances

- **CON-1:** Un script shell est autorisé par conception : la sandbox est un shell.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose
- **CON-2:** Pas de port dédié : routes sur le port principal, sous le jeton de l'Owner, comme `/git/*` et `/mcp`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose ; SpecDD /sandbox/src/lib.sdd
- **CON-3:** Le callback exige un step avec accès au `Context` Restate ; `MiryadWorkflowStep::run` de miryad-core 0.1.4 n'en offre pas.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose et Tasks
- **CON-4:** L'état des jobs est en mémoire ; un redémarrage du pod tue les processus.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Handles
- **Dépendances :** `SpecDD /sandbox/src/venv.sdd` (overlay `.venv`), `SpecDD /sandbox/src/tools_impl.sdd` (`confine`), `SpecDD /sandbox/src/auth.sdd` (`require_auth`), `SpecDD /tools/src/command.sdd` (tuer le groupe, R8), `SpecDD /sandbox/src/config.sdd`, `SpecDD /sandbox/src/health.sdd`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Can read et Handles

## Exigences fonctionnelles

- **FR-1 - MUST:** `POST /jobs` lance `sh -c <script>` dans un groupe de processus dédié, `cwd` confiné, environnement hérité + overlay `.venv` + `env` du corps (dernière valeur gagnante) ; la sortie va dans un fichier de log unique, non borné à l'écriture.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes et Handles ; **Verified by:** `AC-1`
- **FR-2 - MUST:** `POST /jobs` avec un `job_id` connu répond 200 avec l'état courant et ne lance jamais un second processus.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes et Must ; Scenario `idempotence_sur_job_id` ; **Verified by:** `AC-2`
- **FR-3 - MUST:** `GET /jobs/{job_id}` rend un `@JobStatus` dont `state` vaut `running`, `succeeded` (code 0), `failed` (code non nul ou signal), `timed_out` ou `cancelled`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes ; **Verified by:** `AC-1`
- **FR-4 - MUST:** `GET /jobs/{job_id}/logs` rend les logs à partir d'`offset`, au plus `limit` octets, avec `X-Log-Size`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes ; **Verified by:** `AC-5`
- **FR-5 - MUST:** `DELETE /jobs/{job_id}` tue le groupe (SIGTERM, SIGKILL après 5 s) et rend `cancelled` ; sur un job terminé, sans effet.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes et Must ; **Verified by:** `AC-3`
- **FR-6 - MUST:** `DELETE /jobs?prefix=` annule les jobs `running` dont l'identifiant commence par le préfixe ; un préfixe vide est refusé en 400.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes ; **Verified by:** `AC-3`
- **FR-7 - MUST:** `timeout_secs` absent vaut 3 600 s ; au-delà de 24 h, ramené à 24 h. À l'échéance, groupe tué, état `timed_out`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes (ratifié S6a) et Handles ; **Verified by:** `AC-4`
- **FR-8 - MUST:** Au plus 4 jobs `running` ; un `job_id` inconnu au plafond rend 429. Vérification et insertion sous un seul verrou.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Handles ; **Verified by:** `AC-2`
- **FR-9 - MUST:** Le registre conserve les 20 derniers jobs terminés ; le plus ancien est évincé avec son log.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Handles ; **Verified by:** `AC-9`
- **FR-10 - MUST:** Si `callback.awakeable_id` est fourni et `restate_ingress_url` configurée, la sandbox appelle `POST {restate_ingress_url}/restate/awakeables/{id}/resolve` à l'état terminal, avec relances (backoff plafonné à 30 s, 10 min au plus) jusqu'à un 2xx.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Handles et Exposes (`@CallbackTiming`) ; **Verified by:** `AC-6`
- **FR-11 - MUST:** `callback` sans `restate_ingress_url` configurée rend 400, sans créer de job.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Handles ; Scenario `callback_sans_url_configuree` ; **Verified by:** `AC-6`
- **FR-12 - MUST:** Un `job_id` inconnu du processus courant ou évincé rend 404 ; le worker fait échouer le nœud.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Exposes et Tasks (DÉCIDÉ 2026-10-14) ; **Verified by:** `AC-8`
- **FR-13 - MUST:** Toutes les routes `/jobs*` sont protégées par `@auth::require_auth` : sans jeton 401, jeton d'un autre Owner 403.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must et Scenario `auth_exigee` ; SpecDD /sandbox/src/auth.sdd ; **Verified by:** `AC-7`

## Sécurité

- **NFR-1 - MUST:** Le contenu du script, de `env` et des logs n'est jamais journalisé.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must ; **Verified by:** voir OI-11
- **NFR-2 - MUST:** Le contenu de `env` n'apparaît ni dans un `@JobStatus` ni dans une erreur.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must not ; **Verified by:** voir OI-11
- **NFR-3 - MUST:** Le callback n'appelle que `restate_ingress_url` ; l'URL n'est jamais fournie par le client, et une redirection vers un autre hôte n'est pas suivie.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must, Must not et Handles ; **Verified by:** `AC-6`
- **NFR-4 - MUST:** `cwd` reste sous `sandbox_root` ; chemin absolu, `..` ou répertoire inexistant rendent 400.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must et Exposes ; **Verified by:** `AC-7`
- **NFR-5 - MUST:** Lecture de logs bornée à 64 KiB ; `log_tail` borné à 4 KiB.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must et Exposes ; **Verified by:** `AC-5`

Hérité sans nouvelle exigence : le script tourne avec les droits du pod, clé SSH comprise (`sandbox#DEC-2`) ; `/jobs` ne borne pas ses effets (`sandbox#DEC-3`). Le `job_id` (charset `[A-Za-z0-9._:-]`, 128 octets au plus, ni `.` ni `..`) entre dans le nom du fichier de log ; l'absence d'évasion par ce chemin relève de la lecture du code, sans Scenario. Modèle de menace : `sandbox#OI-1`.

## Conception

Cycle de vie :

```text
POST /jobs (nouveau job_id, sous plafond) -> running
running -> succeeded | failed | timed_out | cancelled (DELETE : SIGTERM, SIGKILL après 5 s)
terminal -> callback (si configuré : résolution d'awakeable, relances)
```

Ordre d'un `POST /jobs`, tel que `handle_create` (`sandbox/src/jobs.rs`) le traite : validation, idempotence (200 sans contrôle du plafond), plafond (429), insertion sous verrou, lancement.

Composants : registre `Jobs` dans `@AppState`, sur le modèle de `@CodeHealth` ; log `<tmp>/vanyline-jobs/<job_id>.log`, supprimé à l'éviction ; boucle de callback avec `@CallbackTiming` injectable (Source: SpecDD /sandbox/src/jobs.sdd, Exposes et Handles).

## Décisions

- **DEC-1 - Contrat asynchrone plutôt qu'appel bloquant**
    - **Context:** un nœud shell dure des heures ; un step Restate ne supporte pas un appel bloquant de cette durée.
    - **Decision:** quatre opérations (démarrage idempotent, état, logs, annulation) appelées depuis un step. **Alternatives:** `execute_command` borné par timeout (écarté).
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose
- **DEC-2 - Idempotence sur un `job_id` fourni par l'appelant**
    - **Context:** un step peut être rejoué après crash du worker.
    - **Decision:** le worker dérive le `job_id` de l'invocation et du step (`<run_id>:<step>`) ; un rejeu répond 200.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose et Exposes
- **DEC-3 - État en mémoire, sans persistance**
    - **Context:** les processus meurent avec le pod.
    - **Decision:** pas de persistance ; un job perdu rend 404 et fait échouer le nœud. **Consequences:** OI-1.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Tasks (DÉCIDÉ 2026-10-14)
- **DEC-4 - Plafonds de départ assumés**
    - **Decision:** 4 `running`, 20 terminés conservés, 3 600 s par défaut, 24 h au maximum ; configurables seulement sur besoin réel (voir OI-3).
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Tasks (DÉCIDÉ 2026-10-14)
- **DEC-5 - Destination de callback fixée par le pod**
    - **Decision:** seule `restate_ingress_url` est appelée. **Alternatives:** URL fournie par le client, écartée par le Must not.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Must et Must not
- **DEC-6 - Script shell autorisé, bornes déléguées au pod**
    - **Decision:** `/jobs` ne filtre pas le contenu du script ; la liste de ce que l'agent ne doit jamais faire est dans `sandbox#OI-1`.
    - **Source:** SpecDD /sandbox/src/jobs.sdd, Purpose ; `sandbox#BR-9`, `sandbox#DEC-3`, `app#DEC-3`

- **DEC-7 - L'idempotence de `POST /jobs` est bornée à la vie du registre**
    - **Context:** le registre est en mémoire ; l'éviction retire les jobs terminés au-delà de 20 (`MAX_FINISHED_KEPT`) ; le nœud `shell` d'`app` rejoue un step après un crash du moteur de workflow.
    - **Decision:** l'idempotence sur `job_id` vaut tant que le job est dans le registre (jobs en cours et 20 jobs terminés), donc pendant la vie du pod. Un rejeu après éviction ou après redémarrage du pod peut relancer le script ; ce risque résiduel est assumé et nommé, et la spec ne promet pas davantage.
    - **Consequences:** l'énoncé « ne lance jamais un second processus » de `SpecDD /sandbox/src/jobs.sdd` est à reformuler.
    - **Source:** décision du développeur, 2026-11-07

## Réalisation SpecDD

- `SpecDD /sandbox/src/jobs.sdd` : `sandbox/src/jobs.rs` (Owns), tests `sandbox/tests/jobs.rs` (FR-1 à FR-12).
- `SpecDD /sandbox/src/config.sdd` : `Config::restate_ingress_url`, `sandbox/src/config.rs` (FR-10, FR-11).
- `SpecDD /sandbox/src/lib.sdd` : montage des routes dans `sandbox/src/lib.rs` (FR-13), non listées dans sa table (OI-5).
- `SpecDD /sandbox/src/auth.sdd` : `require_auth`, codes 401 et 403 (FR-13).
- `SpecDD /app/src/workflow/steps/shell.sdd`, `SpecDD /app/src/workflow/steps/gate.sdd` : consommateurs, lus dans leurs propres Solutions.

## Acceptation et vérification

- **AC-1:** `echo hi; exit 0` rend 201, puis `succeeded`, `exit_code: 0`, `log_tail` contenant `hi`. Scenario `demarrage_puis_lecture_de_l_etat`, `code_de_sortie_non_nul_est_failed`. Vérifie FR-1, FR-3.
- **AC-2:** Un rejeu rend 200 avec le même `@JobStatus`, sans second processus ; au plafond, un `job_id` inconnu rend 429. Scenarios `idempotence_sur_job_id`, `plafond_de_jobs_simultanes`. Vérifie FR-2, FR-8.
- **AC-3:** `DELETE` tue le groupe, un second `DELETE` est sans effet, le préfixe ne touche que ses jobs, un préfixe vide rend 400. Scenarios `annulation`, `annulation_par_prefixe`. Vérifie FR-5, FR-6.
- **AC-4:** À l'échéance, `timed_out` et aucun processus du groupe ne survit. Scenario `timeout_tue_le_groupe`. Vérifie FR-7.
- **AC-5:** Chaque lecture fait au plus 64 KiB, et la concaténation redonne les logs. Scenario `logs_incrementaux_bornes`. Vérifie FR-4, NFR-5.
- **AC-6:** Le callback va vers la seule URL configurée et relance jusqu'au 2xx ; sans URL, 400. Scenarios `callback_vers_restate_configure`, `callback_relance_jusqu_au_succes`, `callback_sans_url_configuree`. Vérifie FR-10, FR-11, NFR-3.
- **AC-7:** `cwd` hors confinement rend 400 ; sans jeton 401, jeton d'un autre Owner 403. Scenarios `cwd_hors_confinement`, `auth_exigee`. Vérifie FR-13, NFR-4.
- **AC-8:** Un `job_id` inconnu rend 404. Scenario `job_perdu_apres_redemarrage`. Vérifie FR-12.
- **AC-9:** Les 20 derniers jobs terminés sont conservés, le plus ancien évincé avec son log. Test d'appoint de `sandbox/tests/jobs.rs`, sans Scenario. Vérifie FR-9.

## Open Issues

- **OI-2 - Rejeu avec corps différent:** la spec ne dit rien d'un `job_id` connu avec un `script`, `cwd` ou `env` différent ; le code rend 200. **TBD :** 200 seulement pour un corps identique, ou 409.
- **OI-3 - Configurabilité du timeout maximal:** `Handles` dit « plafond configurable à 24 h », `Tasks` (DÉCIDÉ 2026-10-14) dit « configurables seulement sur besoin réel ». **TBD :** lequel fait foi.
- **OI-4 - Préfixe absent sur `DELETE /jobs`:** `Exposes` dit « un préfixe vide est refusé » et « préfixe ABSENT = préfixe vide ». La lecture naturelle est 400 ; un préfixe absent annulerait tout sous une autre lecture. **TBD :** écrire le 400.
- **OI-5 - Routes absentes de `lib.sdd`:** la table de routes protégées de `SpecDD /sandbox/src/lib.sdd` ne liste pas `/jobs*`, que `lib.rs` monte. **TBD.**
- **OI-6 - Codes d'erreur:** `sandbox#NFR-1` exige un `VNL-SBX-*` par erreur ; `jobs.sdd` n'en donne aucun (400, 404, 429), alors que le code émet `VNL-SBX-JOBS-009`. **TBD.**
- **OI-7 - Côté contrôleur:** pose de `restate_ingress_url` et règle d'egress vers Restate non cochées (`SpecDD /controller/src/sandbox.sdd`, `SpecDD /controller/src/application.sdd`). **TBD :** état réel, non vérifié ici.
- **OI-8 - Consommateurs `app` non confirmés:** `dsd/app.md` présente les kinds `shell` et `gate` comme « Ajout proposé par Claude, à confirmer ». **TBD.**
- **OI-9 - Surcharge de l'environnement de toolchain:** le `env` du corps peut-il surcharger `PATH` ou `LD_LIBRARY_PATH` (`sandbox#BR-5`) ? Non dit. **TBD.**
- **OI-10 - Logs non bornés à l'écriture:** un job peut saturer le disque du pod ; la borne relève de `sandbox#OI-1`. **TBD :** renvoi à confirmer.
- **OI-11 - Vérification de NFR-1 et NFR-2:** aucun Scenario ne vérifie la non-journalisation ni l'absence de `env` dans les réponses. **TBD.**

## Références

- `sandbox` : `sandbox#BO-1`, `sandbox#FR-4`, `sandbox#BR-1`, `sandbox#BR-5`, `sandbox#BR-6`, `sandbox#BR-7`, `sandbox#BR-9`, `sandbox#NFR-1`, `sandbox#DEC-2`, `sandbox#DEC-3`, `sandbox#OI-1` (`dsd/sandbox.md`).
- `app#DEC-3` (`dsd/app.md`) ; SpecDD `/sandbox/src/jobs.sdd`, `/sandbox/src/config.sdd`, `/sandbox/src/lib.sdd`, `/sandbox/src/auth.sdd`, `/app/src/workflow/steps/shell.sdd`, `/app/src/workflow/steps/gate.sdd`.
