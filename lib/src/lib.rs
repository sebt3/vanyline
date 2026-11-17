//! Librairie d'interface avec le cluster et les sandboxes, partagée par `app` et par la CLI :
//! le client Kubernetes ([`k8s`]), le client de l'accès fichier d'une sandbox ([`sandbox_fs`])
//! et l'erreur du crate ([`VnyError`]). Câblage des modules cf. `./lib.sdd`.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

mod error;
#[cfg(feature = "k8s")]
/// Client K8s typé pour les CRDs vanyline (découverte, CRUD namespaced,
/// constructeurs d'URL de sandbox, watch normalisé) — cf. `./k8s.sdd`.
pub mod k8s;
#[cfg(feature = "k8s")]
/// Client de l'accès fichier de la sandbox (ticket + `/ws/fs`) — cf. `./sandbox_fs.sdd`.
pub mod sandbox_fs;

pub use error::*;
