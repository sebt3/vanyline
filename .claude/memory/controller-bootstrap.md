# controller-bootstrap — WS-4 (terminé, clôturé après coup le 2026-07-31)

`vanyline-controller` sorti du statut déféré : trois CRDs v1alpha1
(Owner/Project/Sandbox) et leurs reconcilers (`owner.rs`/`project.rs`/`sandbox.rs`),
7 tâches candidates du design toutes implémentées (crds, owner-reconciler,
project-jobs-builder, project-reconciler, sandbox-pod-builder, sandbox-reconciler,
deploy). Détails architecturaux : `docs/architecture.md` section "Opérateur
Kubernetes — vanyline-controller". `docs/features/controller-bootstrap.md` supprimé
à la clôture.

**Anomalie de process découverte en reprenant cette feature** : le design doc était
resté en `docs/features/` alors que le code était fini, testé (67 tests) et déployé
depuis le 2026-07-11 (image publiée `docker.io/sebt3/vanyline-controller:0.0.1-alpha.1`,
validée en e2e réel sur le cluster de dev — commit `ef0d3da`, qui a d'ailleurs débusqué
un vrai bug de `PatchParams::force` incompatible avec `Patch::Merge` sur le patch de
status). La Phase 3 (clôture : migration vers `architecture.md` + suppression du
design doc) n'avait jamais été faite alors que WS-9 (sandbox-maint-agent) et WS-8
(github-publication) ont ensuite modifié `controller/` sans jamais y toucher — signe
que la clôture peut se perdre silencieusement quand plusieurs features s'enchaînent
sans repasser explicitement par la Phase 3 de chacune. Réflexe à garder : après toute
tâche qui touche un composant dont le design doc est encore présent, vérifier s'il est
encore d'actualité plutôt que de supposer qu'il a déjà été clos.
