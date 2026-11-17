//! Comparaison à trois (copie source, copie de destination, manifeste) et plan d'actions
//! (`cli/src/sync/plan.sdd`). Pure et déterministe : aucune E/S, aucune écriture.

use std::collections::{BTreeMap, BTreeSet};

use lymaille::SecretDetector;

use super::manifest::Manifest;
use super::tree::{RelPath, Tree};

/// Sens de la synchronisation. La source est le côté dont la copie gagne en cas de `force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Local vers sandbox (`sync push`).
    Push,
    /// Sandbox vers local (`sync pull`).
    Pull,
}

/// Action pour un chemin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// Rien à faire : les deux copies sont identiques, ou la destination n'a qu'un fichier que `no_delete` protège.
    Nothing,
    /// Créer dans la destination.
    Create,
    /// Remplacer le contenu de la destination par celui de la source.
    Update,
    /// Supprimer de la destination.
    Delete,
    /// Les deux copies ont divergé : rien n'est écrit sans `force`.
    Diverged,
}

/// Options du plan (`--force-local` / `--force-remote`, `--no-delete`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanOptions {
    /// Trancher en faveur de la source en cas de divergence.
    pub force: bool,
    /// Ne jamais supprimer dans la destination.
    pub no_delete: bool,
}

/// Plan d'une synchronisation : une action par chemin, et les chemins de la source qui contiennent un secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Sens de la synchronisation.
    pub direction: Direction,
    /// Action par chemin, dans l'ordre des chemins.
    pub actions: BTreeMap<RelPath, Action>,
    /// Fichiers de la source qui contiennent une valeur reconnue comme secret (vide si la synchronisation peut
    /// s'exécuter). Renseigné par l'appelant avec [`find_secrets`].
    pub secrets: Vec<RelPath>,
}

/// Calcule le plan. `source` et `destination` sont les arbres des deux côtés, `manifest` la liste de la
/// dernière synchronisation réussie.
#[must_use]
pub fn build_plan(
    direction: Direction,
    source: &Tree,
    destination: &Tree,
    manifest: &Manifest,
    options: PlanOptions,
) -> Plan {
    let actions = chemins_connus(source, destination, manifest)
        .into_iter()
        .map(|chemin| {
            let action = decider(
                source.get(&chemin).map(|e| e.sha256),
                destination.get(&chemin).map(|e| e.sha256),
                manifest.get(&chemin).copied(),
                options,
            );
            (chemin, action)
        })
        .collect();
    Plan {
        direction,
        actions,
        secrets: Vec::new(),
    }
}

/// Table de décision pour un chemin. `s`, `d` et `m` sont les empreintes de la source, de la destination et du
/// manifeste (`None` = absent). La destination « n'a pas changé » quand elle est égale au manifeste.
fn decider(
    s: Option<[u8; 32]>,
    d: Option<[u8; 32]>,
    m: Option<[u8; 32]>,
    options: PlanOptions,
) -> Action {
    match (s, d) {
        (None, None) => Action::Nothing,
        (Some(es), Some(ed)) if es == ed => Action::Nothing,
        (Some(_), Some(ed)) => {
            if m == Some(ed) || options.force {
                Action::Update
            } else {
                Action::Diverged
            }
        }
        (Some(_), None) => {
            // Jamais synchronisé : création. Sinon la destination a été supprimée depuis : divergence.
            if m.is_none() || options.force {
                Action::Create
            } else {
                Action::Diverged
            }
        }
        (None, Some(ed)) => {
            if options.no_delete {
                Action::Nothing
            } else if m == Some(ed) || options.force {
                Action::Delete
            } else {
                Action::Diverged
            }
        }
    }
}

/// Chemins de la source qui contiennent une valeur que le détecteur reconnaît (`sync#FR-6`).
#[must_use]
pub fn find_secrets(source: &Tree, detector: &impl SecretDetector) -> Vec<RelPath> {
    source
        .iter()
        .filter(|(_, entree)| contient_secret(&entree.octets, detector))
        .map(|(chemin, _)| chemin.clone())
        .collect()
}

/// Vrai si une ligne ou un jeton (séparé par espace, `=`, `:`, guillemet…) est reconnu comme secret. Un
/// fichier qui n'est pas du texte UTF-8 est tenu pour suspect (le détecteur ne lit que du texte).
fn contient_secret(octets: &[u8], detector: &impl SecretDetector) -> bool {
    let Ok(texte) = std::str::from_utf8(octets) else {
        return true;
    };
    texte.lines().any(|ligne| {
        detector.looks_like_secret(ligne.trim())
            || ligne
                .split(|c: char| c.is_whitespace() || "=:\"',;".contains(c))
                .any(|jeton| detector.looks_like_secret(jeton))
    })
}

impl Plan {
    /// Vrai si au moins un chemin a divergé.
    #[must_use]
    pub fn has_divergence(&self) -> bool {
        self.actions.values().any(|a| *a == Action::Diverged)
    }

    /// Vrai si aucune action n'est à faire.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.values().all(|a| *a == Action::Nothing)
    }

    /// Liste lisible des chemins à créer, modifier, supprimer ou qui ont divergé. Sans contenu de fichier.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut lignes = Vec::new();
        for (chemin, action) in &self.actions {
            let libelle = match action {
                Action::Nothing => continue,
                Action::Create => "créer",
                Action::Update => "modifier",
                Action::Delete => "supprimer",
                Action::Diverged => "DIVERGE",
            };
            lignes.push(format!("  {libelle:<10} {chemin}"));
        }
        lignes.join("\n")
    }
}

/// Chemins présents dans l'une des trois sources : source, destination ou manifeste.
fn chemins_connus(source: &Tree, destination: &Tree, manifest: &Manifest) -> BTreeSet<RelPath> {
    let mut chemins: BTreeSet<RelPath> = source.keys().cloned().collect();
    chemins.extend(destination.keys().cloned());
    chemins.extend(manifest.chemins().cloned());
    chemins
}

#[cfg(test)]
mod tests {
    use lymaille::DefaultSecretDetector;

    use super::*;
    use crate::sync::tree::{Entry, empreinte};

    fn arbre(entrees: &[(&str, &str)]) -> Tree {
        entrees
            .iter()
            .map(|(chemin, texte)| {
                (
                    RelPath::new(chemin).expect("chemin"),
                    Entry::new(texte.as_bytes().to_vec()),
                )
            })
            .collect()
    }

    fn rel(texte: &str) -> RelPath {
        RelPath::new(texte).expect("chemin")
    }

    /// Manifeste contenant `chemin` avec l'empreinte de `texte`.
    fn manifeste(chemin: &str, texte: &str) -> Manifest {
        let mut m = Manifest::new();
        m.record(rel(chemin), empreinte(texte.as_bytes()));
        m
    }

    fn options(force: bool, no_delete: bool) -> PlanOptions {
        PlanOptions { force, no_delete }
    }

    /// Scenario `divergence` : modifié des deux côtés depuis la dernière synchronisation.
    #[test]
    fn divergence() {
        let source = arbre(&[("a.md", "local")]);
        let destination = arbre(&[("a.md", "distant")]);
        let m = manifeste("a.md", "ancien");
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Diverged));
        assert!(plan.has_divergence());
        assert!(!plan.is_empty());
    }

    /// Scenario `force en faveur de la source` : même divergence avec `force`.
    #[test]
    fn force_en_faveur_de_la_source() {
        let source = arbre(&[("a.md", "local")]);
        let destination = arbre(&[("a.md", "distant")]);
        let m = manifeste("a.md", "ancien");
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(true, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Update));
        assert!(!plan.has_divergence());
    }

    /// Scenario `suppression propagée ou non` : supprimé en source depuis la dernière synchronisation.
    #[test]
    fn suppression_propagee_ou_non() {
        let source = arbre(&[]);
        let destination = arbre(&[("a.md", "x")]);
        let m = manifeste("a.md", "x");
        let propage = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, false),
        );
        assert_eq!(propage.actions.get(&rel("a.md")), Some(&Action::Delete));
        let garde = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, true),
        );
        assert_eq!(garde.actions.get(&rel("a.md")), Some(&Action::Nothing));
    }

    /// Scenario `première synchronisation` : manifeste vide, destination sans le fichier.
    #[test]
    fn premiere_synchronisation() {
        let source = arbre(&[("a.md", "x")]);
        let destination = arbre(&[]);
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &Manifest::new(),
            options(false, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Create));
    }

    /// Scenario `secret détecté` : une valeur reconnue par le détecteur est rendue, avec son chemin.
    #[test]
    fn secret_detecte() {
        let source = arbre(&[
            ("propre.md", "rien à voir\n"),
            ("cle.md", "sk-abcdef0123456789\n"),
            ("entete.md", "Authorization: Bearer abc\n"),
        ]);
        let secrets = find_secrets(&source, &DefaultSecretDetector::default());
        assert!(secrets.contains(&rel("cle.md")), "obtenu : {secrets:?}");
        assert!(secrets.contains(&rel("entete.md")), "obtenu : {secrets:?}");
        assert!(!secrets.contains(&rel("propre.md")));
    }

    /// Scenario `rien à faire` : deux copies identiques.
    #[test]
    fn rien_a_faire() {
        let source = arbre(&[("a.md", "x")]);
        let destination = arbre(&[("a.md", "x")]);
        let plan = build_plan(
            Direction::Pull,
            &source,
            &destination,
            &Manifest::new(),
            options(false, false),
        );
        assert!(plan.actions.values().all(|a| *a == Action::Nothing));
        assert!(plan.is_empty());
        assert!(!plan.has_divergence());
    }

    /// Seule la source a changé depuis la dernière synchronisation : mise à jour, pas de divergence.
    #[test]
    fn seule_la_source_a_change() {
        let source = arbre(&[("a.md", "nouveau")]);
        let destination = arbre(&[("a.md", "ancien")]);
        let m = manifeste("a.md", "ancien");
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Update));
    }

    /// Seule la destination a changé (fichier édité dans la sandbox) : divergence, la source ne l'écrase pas.
    #[test]
    fn seule_la_destination_a_change() {
        let source = arbre(&[("a.md", "ancien")]);
        let destination = arbre(&[("a.md", "edite-dans-la-sandbox")]);
        let m = manifeste("a.md", "ancien");
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Diverged));
    }

    /// Fichier présent seulement en destination, jamais synchronisé : divergence ; `no_delete` le laisse.
    #[test]
    fn fichier_seulement_en_destination() {
        let source = arbre(&[]);
        let destination = arbre(&[("a.md", "x")]);
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &Manifest::new(),
            options(false, false),
        );
        assert_eq!(plan.actions.get(&rel("a.md")), Some(&Action::Diverged));
        let garde = build_plan(
            Direction::Push,
            &source,
            &destination,
            &Manifest::new(),
            options(false, true),
        );
        assert_eq!(garde.actions.get(&rel("a.md")), Some(&Action::Nothing));
    }

    /// Le résumé liste les chemins et jamais le contenu des fichiers.
    #[test]
    fn resume_sans_contenu() {
        let source = arbre(&[("nouveau.md", "CONTENU-TRES-PRIVE")]);
        let destination = arbre(&[("vieux.md", "AUTRE-CONTENU")]);
        let m = manifeste("vieux.md", "AUTRE-CONTENU");
        let plan = build_plan(
            Direction::Push,
            &source,
            &destination,
            &m,
            options(false, false),
        );
        let resume = plan.summary();
        assert!(resume.contains("nouveau.md"), "résumé : {resume}");
        assert!(resume.contains("vieux.md"), "résumé : {resume}");
        assert!(!resume.contains("CONTENU-TRES-PRIVE"));
        assert!(!resume.contains("AUTRE-CONTENU"));
    }
}
