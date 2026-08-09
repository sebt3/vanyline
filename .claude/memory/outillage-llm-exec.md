# Outillage — délégation à Qwen via `llm-exec`

Depuis la feature cli-rpc-stdio (2026-07-12, 6 tâches + plusieurs rondes
de correction), Claude délègue directement à Qwen via `llm-exec` (plus de
passe-plat humain) : fichier de tâche écrit dans `.tasks/<feature>/`,
lancé en arrière-plan, revu et validé par Claude après coup, committé par
Claude. Règles stabilisées :

- **Toujours préfixer `env -u OPENCODE_SERVER_PASSWORD -u
  OPENCODE_BINARY`** devant `llm-exec`/`opencode run` lancé depuis le Bash
  tool de Claude Code — sans ça, `opencode run` échoue avec `Session not
  found` (l'environnement hérite ces deux variables du pod code-server ;
  un terminal interactif classique n'a pas le problème). Non lié à un
  process `opencode serve` particulier — c'est la présence des variables
  elles-mêmes qui casse la création de session.
- **Modèle** : `-m "strix/qwen3.6:35b-a3b"` (MoE) par défaut sur ce
  projet, pas le dense `27b` de la consigne globale — trop lent en
  pratique sur le Strix (préférence du développeur). Toujours passer `-m`
  explicitement, ne jamais compter sur l'auto-découverte.
- **Cause racine trouvée et corrigée (2026-07-31)** : les permissions
  `bash: ... allow` de `.opencode/agents/implement.md` n'avaient aucun
  effet parce que le catch-all `"*": ask` était placé en **dernière**
  position du mapping YAML — or la résolution de permission d'opencode
  applique la règle **la dernière qui matche gagne** (confirmé par la doc
  officielle : "Rules are evaluated by pattern match, with the last
  matching rule winning" ; pattern recommandé : catch-all en premier,
  règles spécifiques après). Un `"*": ask` en fin de liste matche tout et
  écrase silencieusement tous les `allow` déclarés au-dessus. Le bloc
  `external_directory` du même fichier avait la bonne structure
  (catch-all d'abord) — c'était une erreur d'ordre isolée au bloc `bash`,
  pas une limitation d'`opencode run` headless. Décision du développeur
  (2026-07-31) : basculer `implement.md` en modèle blacklist — `bash:
  "*": allow` en tête, puis `git push*: deny`, `git rebase*: deny`,
  `sudo*: deny` après. Qwen peut désormais committer et faire tout ce que
  le développeur ferait en bash, sauf push/rebase/sudo — **changement de
  workflow assumé** : l'ancienne règle "Claude committe systématiquement
  après relecture" ne s'applique plus par défaut à `implement`, seule la
  relecture (`cargo check/test/clippy` après chaque délégation) reste
  systématique. `diagnose.md` avait le même bug d'ordre sur son bloc
  `bash` (`"*": deny` en dernier) — corrigé en gardant le modèle whitelist
  (catch-all `deny` remis en premier) car `diagnose` a `edit: deny` et ne
  doit jamais pouvoir écrire de fichier, y compris via un détour bash
  (`sed -i`, redirection `>`) — un blacklist y créerait une fuite de la
  garantie "aucune modification".
- **`external_directory: /home/coder/.config/opencode/*: allow`** ajouté
  aux deux agents (2026-07-31) : Qwen plantait en boucle sur un `ask`
  auto-rejeté en tentant de lire `~/.config/opencode/AGENTS.md` (contexte
  global qu'il connaît par sa propre config opencode). Décision du
  développeur : autoriser quand même, malgré ce fichier contenant un prénom
  en clair (que la règle globale du développeur garde hors des fichiers
  commités) — Qwen est un outil d'exécution locale, pas un tiers externe ;
  le risque réel est qu'il reproduise ce prénom dans un fichier commité ou
  un message généré, à surveiller en review plutôt qu'à bloquer en amont.
- **Une permission auto-rejetée (bash OU external_directory) a une chance
  non négligeable de faire planter toute la session `opencode run`**
  plutôt que de laisser Qwen s'adapter et continuer — observé à 3 reprises
  sur cette feature (deux fois sur des `bash find` avant le fix d'ordre,
  une fois sur un `external_directory` légitimement `ask`). Le fix d'ordre
  bash + l'ajout d'`external_directory` réduisent la fréquence des
  rejets, mais ne garantissent pas qu'un rejet restant (ex: un chemin
  externe vraiment hors périmètre) laisse la session continuer proprement.
  Réflexe : en cas de session qui se termine avec un diff vide ou quasi
  vide après un `! permission requested ... auto-rejecting` dans le log,
  relancer directement la même tâche plutôt que de chercher un bug dans le
  fichier de tâche — c'est souvent juste ce plantage.
- **Corriger en déléguant, pas en patchant direct** (retour du
  développeur, 2026-07-12) : un bug/oubli remonté par la validation de
  Claude devient un fichier de tâche de correction (`<tâche>-fix-NN.md`)
  délégué à Qwen, jamais un `Edit` direct de Claude — sauf blocage réel
  d'outillage où Qwen ne peut de toute façon rien faire de plus (rare : la
  plupart des bugs de code s'y prêtent très bien, vu sur 4 rondes de
  correction consécutives sur `chat/send` sans encombre).
- **Qwen oublie régulièrement d'écrire les tests prévus par la tâche**
  même quand l'implémentation de production est correcte (vu sur 2 tâches
  sur 6) — vérifier le compte de tests avant/après (`cargo test
  --workspace`) plutôt que de faire confiance au rapport final de Qwen ;
  si des tests manquent, fichier de tâche de correction dédié plutôt que
  de les écrire soi-même.
- **Qwen ne respecte pas fiablement le format de message de commit** donné
  verbatim dans la section "Commit" du fichier de tâche (vu sur
  ws08-github-publication, 2026-07-31 : 4 commits sur 7 dans le mauvais
  format — `feat(nom): titre` ou `feat: nom — titre` au lieu de
  `(feat: nom) titre — description`, parfois même sans le nom de la feature
  du tout). Sans conséquence fonctionnelle mais casse la cohérence de
  l'historique. Comme rien n'est poussé avant la fin de la feature (`origin/main`
  très en retard sur ce projet solo), corriger via `git commit --amend`
  (commit de tête) ou `git reset --soft` + recommit (commits plus anciens)
  est sûr — **toujours vérifier le format après chaque délégation**, ne pas
  supposer que la consigne verbatim suffit.
- **Qwen peut stager plus large que le périmètre de la tâche** (vu sur
  ws08-github-publication, tâche `dockerfiles` : le premier commit a
  embarqué `docs/roadmap-sprint2.md`, un fichier non tracké sans rapport,
  probablement via un `git add` trop large côté Qwen). Corrigé une fois
  (reset + recommit ciblé) puis **prévenu en ajoutant une consigne
  explicite dans chaque tâche suivante** ("stager précisément les fichiers
  listés, jamais `git add -A`/`git add .`") — n'a plus reproduit sur les 6
  tâches suivantes. Instruction à inclure systématiquement dans la section
  "Commit" de tout fichier de tâche sur ce projet, tant que le repo contient
  des design docs non trackés en attente (`docs/features/*.md`,
  `docs/roadmap*.md`).
- **`external_directory` en dehors du repo courant peut planter toute la
  session même avec la whitelist déjà élargie** (confirmé à nouveau sur
  ws08-github-publication, tâche `ci-test` v1, 2026-07-31) : un fichier de
  tâche qui demandait à Qwen de lire `~/projets/juke/.github/workflows/`
  (repo voisin, hors périmètre `implement.md`) a fait planter la session sur
  un rejet auto (diff vide, aucun commit). Fix : **ne jamais référencer un
  chemin hors du repo courant dans un fichier de tâche** — si du contenu
  d'un autre projet sert de modèle, le lire soi-même (Claude) et le
  reproduire intégralement dans la section "Code partiel" du fichier de
  tâche plutôt que de renvoyer Qwen le lire.
- **Le compte de tests annoncé par Qwen dans son propre rapport peut être
  faux même quand l'exécution réelle est correcte** (vu sur
  ws08-github-publication, tâche `fmt-repo` : Qwen a annoncé "363 tests
  passés", le vrai chiffre — revérifié par Claude — était 474, identique à
  avant la tâche, aucune régression). Ne jamais citer le chiffre du rapport
  Qwen sans le revérifier soi-même via `cargo test --workspace`.
- **`AGENTS.md` documente des commandes de validation moins strictes que ce
  qu'il faudrait pour une CI propre** : `cargo clippy --workspace` (sans
  `--all-targets`) ne vérifie pas le code de test, jamais remarqué avant
  d'écrire une vraie CI (ws08-github-publication, 2026-07-31) — a révélé
  d'un coup ~20 erreurs préexistantes (surtout un faux positif
  `await_holding_lock` répété sur le pattern `isolated_data_dir()`, cf.
  `cli-rpc-stdio.md`). Idem `cargo fmt --all --check` : jamais
  lancé sur tout le workspace, 53 fichiers non conformes découverts d'un
  coup. Les deux ont nécessité une tâche de nettoyage dédiée avant que la CI
  parte verte. Leçon : **une commande de validation documentée mais jamais
  réellement exécutée sur tout le périmètre n'est pas une garantie** — le
  premier run réel révèle souvent de la dette accumulée silencieusement.
- **Récidive du piège `/tmp` hors whitelist** (ws13-sandbox-runtime, tâche
  `image-cmds`, 2026-08-01) : un fichier de tâche qui demandait à Qwen
  d'écrire un Dockerfile de validation sous `/tmp/<nom>/` (au lieu de
  `/tmp/opencode/*`, seul préfixe `/tmp` whitelisté dans
  `external_directory` de `implement.md`) a fait échouer le `mkdir`
  (`auto-rejecting`) — la session ne plante pas cette fois (juste le step
  de validation qui ne s'exécute pas), mais rien n'est commité. Déjà
  documenté une fois pour de la *lecture* hors repo (cf. plus haut,
  ws08) ; ici c'est de l'*écriture* d'un fichier scratch, même cause
  racine. **Réflexe à appliquer systématiquement en rédigeant un fichier
  de tâche** : tout chemin scratch/temporaire donné à Qwen doit être soit
  `/tmp/opencode/*`, soit un chemin dans le repo (ex.
  `.tasks/<feature>/scratch/`) — jamais un `/tmp/<nom-libre>/` inventé au
  moment de la rédaction. Traité comme blocage d'outillage réel (pas un
  bug de code) : validé et committé directement par Claude plutôt que
  re-délégué.

## Agent opencode `cadence` (2026-08-03)

Copié depuis `kydah/vyrn` (`.opencode/agents/cadence.md`, seule adaptation : la mention de
projet dans le corps du prompt, `vyrn` → `vanyline`) — le reste (permissions, workflow)
correspondait déjà exactement au workflow de ce projet (mêmes Phases 1/2/3, même format de
fichier de tâche, agents `implement`/`diagnose` déjà présents sous les mêmes noms). Rôle :
cadence l'implémentation d'une feature déjà designée — découpe en tâches just-in-time,
dispatch à `implement`, valide avant de passer à la suivante ; ne conçoit pas l'architecture
et ne fait pas la revue finale (réservées à Claude, Phases 1 et 3).

## Nouveaux modes d'échec Qwen observés (complètent la liste ci-dessus)

Vus sur `ws09-sandbox-maint-agent.md` et `ws12-sandbox-clients.md` — apostrophes françaises
dans un message de commit, corruption de code sans rapport en éditant maladroitement autour,
format de commit ignoré même avec la procédure `-F`, valeur d'exemple sensible recopiée dans
la doc générée, compaction de contexte mi-session. Détails dans les fichiers dédiés
correspondants (répétés là où ils ont été rencontrés en premier).
