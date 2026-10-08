//! Outils MCP partagés du workspace vanyline : exécution de commandes
//! bornées (`command`), opérations sur le système de fichiers (`filesystem`),
//! recherche de fichiers et de motifs (`search`), bornage de sortie
//! (`output`) et déclarations JSON des schémas d'outils (`mcp`). Erreur
//! unique à identifiants stables `VNL-TLS-*` (`error`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

/// Exécution de commandes shell bornées (timeout, sortie bornée tête+queue).
pub mod command;
/// Erreur unique du crate, à identifiants stables `VNL-TLS-*`.
pub mod error;
/// Outils sur le système de fichiers : lecture/écriture atomique, édition par
/// motif, listage borné, suppression, mkdir, renommage.
pub mod filesystem;
/// Déclarations JSON des outils MCP (`filesystem_tools`, `search_tools`,
/// `command_tools`) : schémas d'arguments exposés aux clients.
pub mod mcp;
/// Bornes de sortie centralisées et bornage (lignes, octets, tête+queue).
pub mod output;
/// Recherche de fichiers par glob et de motifs par regex dans une arborescence.
pub mod search;
