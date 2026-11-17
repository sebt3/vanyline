//! Liste des empreintes à la dernière synchronisation réussie, par couple dépôt et sandbox
//! (`cli/src/sync/manifest.sdd`, `sync#FR-3`, `FR-9`). Aucun contenu de fichier n'y figure.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::host::HostError;
use crate::host::files;
use crate::host::layers::Layers;

use super::tree::{RelPath, empreinte};

/// Empreintes SHA-256 des fichiers copiés à la dernière synchronisation, par chemin relatif.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    entrees: BTreeMap<RelPath, [u8; 32]>,
}

impl Manifest {
    /// Manifeste vide.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge le manifeste du couple (`depot`, `sandbox`) ; fichier absent : manifeste vide.
    ///
    /// # Errors
    ///
    /// [`HostError`] si le fichier existe mais ne se lit pas ou n'est pas un manifeste valide.
    pub fn load(layers: &Layers, depot: &Path, sandbox: &str) -> Result<Self, HostError> {
        let chemin = chemin_manifeste(layers, depot, sandbox);
        let Some(texte) = files::read_text(&chemin)? else {
            return Ok(Self::new());
        };
        let brut: BTreeMap<String, String> =
            serde_json::from_str(&texte).map_err(|erreur| HostError::Format {
                chemin: chemin.clone(),
                cause: format!("manifeste de synchronisation invalide : {erreur}"),
            })?;
        let mut entrees = BTreeMap::new();
        for (texte_chemin, texte_empreinte) in brut {
            let format_err = |cause: String| HostError::Format {
                chemin: chemin.clone(),
                cause,
            };
            let chemin_rel = RelPath::new(&texte_chemin)
                .map_err(|erreur| format_err(format!("chemin invalide : {erreur}")))?;
            let empreinte = depuis_hex(&texte_empreinte)
                .ok_or_else(|| format_err("empreinte invalide".to_owned()))?;
            entrees.insert(chemin_rel, empreinte);
        }
        Ok(Self { entrees })
    }

    /// Sauvegarde le manifeste du couple, par écriture atomique, sous `<state>/sync/`.
    ///
    /// # Errors
    ///
    /// [`HostError`] si l'écriture échoue.
    pub fn save(&self, layers: &Layers, depot: &Path, sandbox: &str) -> Result<(), HostError> {
        let chemin = chemin_manifeste(layers, depot, sandbox);
        let brut: BTreeMap<String, String> = self
            .entrees
            .iter()
            .map(|(chemin_rel, empreinte)| (chemin_rel.to_string(), en_hex(empreinte)))
            .collect();
        let texte = serde_json::to_string_pretty(&brut).map_err(|erreur| HostError::Format {
            chemin: chemin.clone(),
            cause: format!("manifeste de synchronisation non sérialisable : {erreur}"),
        })?;
        files::write_atomic(&chemin, texte.as_bytes(), 0o600)
    }

    /// Enregistre l'empreinte d'un fichier copié.
    pub fn record(&mut self, chemin: RelPath, sha256: [u8; 32]) {
        self.entrees.insert(chemin, sha256);
    }

    /// Oublie un fichier (supprimé).
    pub fn forget(&mut self, chemin: &RelPath) {
        self.entrees.remove(chemin);
    }

    /// Empreinte enregistrée pour `chemin`.
    #[must_use]
    pub fn get(&self, chemin: &RelPath) -> Option<&[u8; 32]> {
        self.entrees.get(chemin)
    }

    /// Chemins enregistrés, dans l'ordre.
    pub fn chemins(&self) -> impl Iterator<Item = &RelPath> {
        self.entrees.keys()
    }
}

/// Fichier du manifeste : `<state>/sync/<empreinte-du-dépôt>-<sandbox>.json` (`manifest.sdd`).
#[must_use]
pub fn chemin_manifeste(layers: &Layers, depot: &Path, sandbox: &str) -> PathBuf {
    let empreinte_depot = en_hex(&empreinte(depot.as_os_str().as_encoded_bytes()));
    // Le nom de la sandbox ne porte que des caractères sûrs : jamais de séparateur de chemin.
    let sandbox_sur: String = sandbox
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    layers
        .state()
        .join("sync")
        .join(format!("{empreinte_depot}-{sandbox_sur}.json"))
}

/// Hexadécimal minuscule d'un tableau d'octets.
fn en_hex(octets: &[u8]) -> String {
    octets.iter().fold(
        String::with_capacity(octets.len().saturating_mul(2)),
        |mut texte, o| {
            let _ = write!(texte, "{o:02x}");
            texte
        },
    )
}

/// Décode 64 caractères hexadécimaux en empreinte ; `None` sinon.
fn depuis_hex(texte: &str) -> Option<[u8; 32]> {
    if texte.len() != 64 {
        return None;
    }
    let mut sortie = [0_u8; 32];
    for (indice, paire) in texte.as_bytes().chunks(2).enumerate() {
        let morceau = std::str::from_utf8(paire).ok()?;
        *sortie.get_mut(indice)? = u8::from_str_radix(morceau, 16).ok()?;
    }
    Some(sortie)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn couche(racine: &Path) -> Layers {
        Layers::discover(racine, racine, None, |_| false)
    }

    fn rel(texte: &str) -> RelPath {
        RelPath::new(texte).expect("chemin valide")
    }

    /// Scenario `aller-retour` : un manifeste enregistré puis rechargé est identique.
    #[test]
    fn aller_retour() {
        let home = tempfile::tempdir().expect("tempdir");
        let depot = home.path().join("depot");
        let layers = couche(home.path());
        let mut m = Manifest::new();
        m.record(rel("agents/a.md"), empreinte(b"a"));
        m.record(rel("config.yaml"), empreinte(b"k: v\n"));
        m.save(&layers, &depot, "alpha").expect("sauvegarde");
        let relu = Manifest::load(&layers, &depot, "alpha").expect("chargement");
        assert_eq!(relu, m);
    }

    /// Scenario `coupure en cours de copie` : trois copies dont la deuxième échoue ; le manifeste
    /// sauvegardé après chaque copie réussie ne mentionne que la première.
    #[test]
    fn coupure_en_cours_de_copie() {
        let home = tempfile::tempdir().expect("tempdir");
        let depot = home.path().join("depot");
        let layers = couche(home.path());
        let mut m = Manifest::new();
        let copies = [
            ("un.md", Ok(())),
            ("deux.md", Err(())),
            ("trois.md", Ok(())),
        ];
        for (chemin, resultat) in copies {
            if resultat.is_ok() {
                m.record(rel(chemin), empreinte(chemin.as_bytes()));
                m.save(&layers, &depot, "alpha").expect("sauvegarde");
            } else {
                break;
            }
        }
        let relu = Manifest::load(&layers, &depot, "alpha").expect("chargement");
        assert_eq!(relu.chemins().count(), 1);
        assert!(relu.get(&rel("un.md")).is_some());
        assert!(relu.get(&rel("deux.md")).is_none());
    }

    /// Fichier absent : manifeste vide, sans erreur.
    #[test]
    fn absent_est_vide() {
        let home = tempfile::tempdir().expect("tempdir");
        let layers = couche(home.path());
        let m = Manifest::load(&layers, &home.path().join("depot"), "alpha").expect("chargement");
        assert!(m.chemins().next().is_none());
    }

    /// Écriture confinée à `<state>/sync/`, nom par empreinte du dépôt et sandbox ; pas de contenu.
    #[test]
    fn ecriture_confinee_a_l_etat_sync() {
        let home = tempfile::tempdir().expect("tempdir");
        let depot = home.path().join("depot");
        let layers = couche(home.path());
        let mut m = Manifest::new();
        m.record(rel("secret.md"), empreinte(b"contenu-secret"));
        m.save(&layers, &depot, "alpha").expect("sauvegarde");
        let chemin = chemin_manifeste(&layers, &depot, "alpha");
        assert!(
            chemin.starts_with(layers.state().join("sync")),
            "chemin : {chemin:?}"
        );
        assert!(chemin.to_string_lossy().ends_with("-alpha.json"));
        let texte = std::fs::read_to_string(&chemin).expect("lecture");
        assert!(texte.contains("secret.md"));
        assert!(!texte.contains("contenu-secret"));
    }

    /// Un manifeste de dépôts différents ne se mélange pas.
    #[test]
    fn depots_distincts() {
        let home = tempfile::tempdir().expect("tempdir");
        let layers = couche(home.path());
        let mut m = Manifest::new();
        m.record(rel("a.md"), empreinte(b"a"));
        m.save(&layers, &home.path().join("depot-1"), "alpha")
            .expect("sauvegarde");
        let autre =
            Manifest::load(&layers, &home.path().join("depot-2"), "alpha").expect("chargement");
        assert!(autre.chemins().next().is_none());
    }
}
