//! Sessions lymaille de l'app : hébergement des sessions par miryad-core — contrat :
//! `mod.sdd`. Le montage complet (routes, fourniture de l'hôte à l'état miryad-core) est B9 ;
//! B6 pose l'hôte lui-même.

#[allow(dead_code)]
// Montage B9 à venir (`./mod.sdd` Tasks) ; host.rs consommé par ses seuls tests — tooling.sdd
mod host;

#[allow(unused_imports)]
// Montage B9 à venir (`./mod.sdd` Tasks) — consommateur : le montage de `main.rs` — tooling.sdd
pub(crate) use host::VnlLymailleHost;

#[allow(dead_code)]
// Montage B9 à venir (`./catalog.sdd` Tasks) ; catalog.rs consommé par ses seuls tests — tooling.sdd
mod catalog;

#[allow(dead_code)]
// Montage B9 à venir (`./mod.sdd` Tasks) ; events.rs consommé par ses seuls tests — tooling.sdd
mod events;
