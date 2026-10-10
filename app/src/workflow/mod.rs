//! Couche workflow de l'app : moteur DAG de miryad-core sur Restate. Squelette W1 : ce fichier
//! ne déclare que les deux modules feuilles livrés (`run_context`, `status`) ; la consistance
//! du module (runtime, steps, câblage) est contractée par `mod.sdd` et arrive avec eux.

#[allow(dead_code)] // W2 consommateur à venir — tooling.sdd
pub mod run_context;
#[allow(dead_code)] // W2 consommateur à venir — tooling.sdd
pub mod status;
