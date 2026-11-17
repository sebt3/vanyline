//! Arbres `.vanyline/` local et distant, lus en octets avec leurs empreintes SHA-256
//! (`cli/src/sync/tree.sdd`). Lecture seule : ce module n'écrit jamais.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;

use sha2::{Digest, Sha256};
use vanyline_lib::sandbox_fs::{FsError, Listing};

/// Nom du dossier synchronisé, à la racine du dépôt local comme à la racine de la sandbox (`sync#FR-1`).
pub const DOSSIER: &str = ".vanyline";

/// Taille maximale d'un fichier synchronisé : 256 Kio (`sync/tree.sdd`, `sync#FR-7`).
pub const TAILLE_MAX: u64 = 262_144;

/// Accès fichier à la sandbox, vu par la synchronisation. `SandboxFs` l'implémente ; les tests
/// utilisent un faux (seam, `sync/command.sdd`).
pub trait DistantFs: Send {
    /// `list` d'un dossier, un niveau.
    fn list(&mut self, chemin: &str) -> impl Future<Output = Result<Listing, FsError>> + Send;
    /// `read` brut d'un fichier.
    fn read_raw(&mut self, chemin: &str) -> impl Future<Output = Result<Vec<u8>, FsError>> + Send;
    /// Écriture atomique d'un fichier, parents créés.
    fn write(
        &mut self,
        chemin: &str,
        octets: &[u8],
    ) -> impl Future<Output = Result<(), FsError>> + Send;
    /// Suppression d'un fichier ; `false` si absent.
    fn delete(&mut self, chemin: &str) -> impl Future<Output = Result<bool, FsError>> + Send;
}

/// Chemin relatif à `.vanyline/`, validé : composantes normales seulement (`sync#FR-7`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Valide un chemin relatif.
    ///
    /// # Errors
    ///
    /// [`TreeError::Path`] si le chemin est vide, absolu, contient `..`, `.`, une composante
    /// vide, un antislash ou un octet nul.
    pub fn new(texte: &str) -> Result<Self, TreeError> {
        let refus = |raison: &'static str| TreeError::Path {
            chemin: texte.to_owned(),
            raison,
        };
        if texte.is_empty() {
            return Err(refus("chemin vide"));
        }
        if texte.contains('\\') || texte.contains('\0') {
            return Err(refus("caractère interdit"));
        }
        if texte.starts_with('/') {
            return Err(refus("chemin absolu"));
        }
        for composante in texte.split('/') {
            match composante {
                "" => return Err(refus("composante vide")),
                "." | ".." => return Err(refus("composante `.` ou `..`")),
                _ => {}
            }
        }
        Ok(Self(texte.to_owned()))
    }

    /// Chemin relatif sous ce dossier, validé comme [`RelPath::new`].
    ///
    /// # Errors
    ///
    /// [`TreeError::Path`] si la composante ajoutée est refusée.
    pub fn joindre(&self, nom: &str) -> Result<Self, TreeError> {
        if nom.contains('/') {
            return Err(TreeError::Path {
                chemin: nom.to_owned(),
                raison: "nom contenant `/`",
            });
        }
        Self::new(&format!("{}/{nom}", self.0))
    }

    /// Texte du chemin, séparateur `/`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Empreinte SHA-256 des octets exacts.
#[must_use]
pub fn empreinte(octets: &[u8]) -> [u8; 32] {
    Sha256::digest(octets).into()
}

/// Un fichier de l'arbre : ses octets exacts et leur empreinte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Octets exacts, saut de ligne final compris.
    pub octets: Vec<u8>,
    /// Empreinte SHA-256 de `octets`.
    pub sha256: [u8; 32],
}

impl Entry {
    /// Entrée pour `octets`, empreinte calculée.
    #[must_use]
    pub fn new(octets: Vec<u8>) -> Self {
        let sha256 = empreinte(&octets);
        Self { octets, sha256 }
    }
}

/// Arbre `.vanyline/` : fichiers par chemin relatif, triés.
pub type Tree = BTreeMap<RelPath, Entry>;

/// Erreurs de lecture d'un arbre. Chacune nomme le chemin en cause.
#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    /// Chemin refusé : sort de `.vanyline/`, lien symbolique, ou nom invalide.
    #[error("VNL-SYNC-001: chemin refusé `{chemin}` : {raison}")]
    Path {
        /// Chemin tel que lu.
        chemin: String,
        /// Motif du refus.
        raison: &'static str,
    },
    /// Fichier qui n'est pas du texte UTF-8.
    #[error("VNL-SYNC-002: `{chemin}` n'est pas du texte UTF-8")]
    NotText {
        /// Chemin du fichier.
        chemin: String,
    },
    /// Fichier au-delà de 256 Kio.
    #[error("VNL-SYNC-003: `{chemin}` dépasse 256 Kio ({octets} octets)")]
    TooLarge {
        /// Chemin du fichier.
        chemin: String,
        /// Taille constatée.
        octets: u64,
    },
    /// Listage distant tronqué : l'arbre serait partiel (`sync#FR-8`).
    #[error("VNL-SYNC-004: listage distant tronqué à 200 entrées dans `{chemin}`")]
    Truncated {
        /// Dossier listé.
        chemin: String,
    },
    /// Lecture locale impossible.
    #[error("VNL-SYNC-005: lecture locale impossible `{chemin}` : {cause}")]
    Local {
        /// Chemin en cause.
        chemin: String,
        /// Cause, sans contenu de fichier.
        cause: String,
    },
    /// Erreur de la sandbox pendant la lecture.
    #[error("VNL-SYNC-006: sandbox : {0}")]
    Distant(#[from] FsError),
}

/// Lit `racine` (le dossier `.vanyline/` local) en arbre. Absent : arbre vide.
///
/// # Errors
///
/// [`TreeError`] pour un chemin refusé, un fichier non texte ou trop gros, ou une lecture impossible.
pub fn read_local(racine: &std::path::Path) -> Result<Tree, TreeError> {
    let mut arbre = Tree::new();
    let meta = match std::fs::symlink_metadata(racine) {
        Ok(meta) => meta,
        Err(erreur) if erreur.kind() == std::io::ErrorKind::NotFound => return Ok(arbre),
        Err(erreur) => return Err(erreur_locale(racine, &erreur)),
    };
    if meta.file_type().is_symlink() {
        return Err(TreeError::Path {
            chemin: DOSSIER.to_owned(),
            raison: "lien symbolique non suivi",
        });
    }
    parcourir_local(racine, None, &mut arbre)?;
    Ok(arbre)
}

/// Erreur de lecture locale nommant le chemin, sans contenu de fichier.
fn erreur_locale(chemin: &std::path::Path, erreur: &std::io::Error) -> TreeError {
    TreeError::Local {
        chemin: chemin.display().to_string(),
        cause: erreur.to_string(),
    }
}

/// Parcours récursif d'un dossier local ; ne suit jamais un lien symbolique.
fn parcourir_local(
    dossier: &std::path::Path,
    rel: Option<&RelPath>,
    arbre: &mut Tree,
) -> Result<(), TreeError> {
    let entrees = std::fs::read_dir(dossier).map_err(|e| erreur_locale(dossier, &e))?;
    for entree in entrees {
        let entree = entree.map_err(|e| erreur_locale(dossier, &e))?;
        let nom = entree
            .file_name()
            .into_string()
            .map_err(|brut| TreeError::Path {
                chemin: brut.to_string_lossy().into_owned(),
                raison: "nom non UTF-8",
            })?;
        let chemin_rel = enfant(rel, &nom)?;
        let absolu = entree.path();
        let meta = std::fs::symlink_metadata(&absolu).map_err(|e| erreur_locale(&absolu, &e))?;
        let type_ = meta.file_type();
        if type_.is_symlink() {
            return Err(TreeError::Path {
                chemin: chemin_rel.to_string(),
                raison: "lien symbolique non suivi",
            });
        } else if type_.is_dir() {
            parcourir_local(&absolu, Some(&chemin_rel), arbre)?;
        } else if type_.is_file() {
            if meta.len() > TAILLE_MAX {
                return Err(TreeError::TooLarge {
                    chemin: chemin_rel.to_string(),
                    octets: meta.len(),
                });
            }
            let octets = std::fs::read(&absolu).map_err(|e| erreur_locale(&absolu, &e))?;
            arbre.insert(chemin_rel.clone(), accepter(&chemin_rel, octets)?);
        } else {
            return Err(TreeError::Path {
                chemin: chemin_rel.to_string(),
                raison: "type de fichier non pris en charge",
            });
        }
    }
    Ok(())
}

/// Chemin d'une entrée sous `rel` (`None` = racine de `.vanyline/`). Un nom qui contient `/` est refusé.
fn enfant(rel: Option<&RelPath>, nom: &str) -> Result<RelPath, TreeError> {
    if nom.contains('/') {
        return Err(TreeError::Path {
            chemin: nom.to_owned(),
            raison: "nom contenant `/`",
        });
    }
    match rel {
        None => RelPath::new(nom),
        Some(parent) => parent.joindre(nom),
    }
}

/// Un fichier accepté : au plus 256 Kio, et du texte UTF-8.
fn accepter(chemin: &RelPath, octets: Vec<u8>) -> Result<Entry, TreeError> {
    let taille = u64::try_from(octets.len()).unwrap_or(u64::MAX);
    if taille > TAILLE_MAX {
        return Err(TreeError::TooLarge {
            chemin: chemin.to_string(),
            octets: taille,
        });
    }
    if std::str::from_utf8(&octets).is_err() {
        return Err(TreeError::NotText {
            chemin: chemin.to_string(),
        });
    }
    Ok(Entry::new(octets))
}

/// Lit `.vanyline/` de la sandbox en arbre, récursivement. Absent : arbre vide.
///
/// # Errors
///
/// [`TreeError`] pour un listage tronqué, un chemin refusé (`../x`), un fichier non texte ou
/// trop gros, ou une erreur de la sandbox.
pub async fn read_remote(fs: &mut impl DistantFs) -> Result<Tree, TreeError> {
    let mut arbre = Tree::new();
    // Pile des dossiers à lister : (chemin distant, chemin relatif ; `None` pour la racine).
    let mut pile: Vec<(String, Option<RelPath>)> = vec![(DOSSIER.to_owned(), None)];
    while let Some((dossier, rel)) = pile.pop() {
        let listage = match fs.list(&dossier).await {
            Ok(listage) => listage,
            Err(FsError::Protocol { ref message, .. })
                if rel.is_none() && message.contains("VNL-TLS-001") =>
            {
                // `.vanyline/` absent côté sandbox : arbre vide (`tree.sdd`).
                return Ok(arbre);
            }
            Err(erreur) => return Err(TreeError::Distant(erreur)),
        };
        if listage.tronque {
            return Err(TreeError::Truncated { chemin: dossier });
        }
        for entree in listage.entrees {
            let chemin_rel = enfant(rel.as_ref(), &entree.nom)?;
            let distant = format!("{dossier}/{}", entree.nom);
            if entree.dossier {
                pile.push((distant, Some(chemin_rel)));
            } else {
                let octets = fs.read_raw(&distant).await?;
                arbre.insert(chemin_rel.clone(), accepter(&chemin_rel, octets)?);
            }
        }
    }
    Ok(arbre)
}

#[cfg(test)]
pub(crate) mod essai {
    //! Faux accès fichier à la sandbox pour les tests de la synchronisation.

    use std::collections::BTreeMap;

    use vanyline_lib::sandbox_fs::{Entree, FsError, Listing};

    use super::DistantFs;

    /// Sandbox en mémoire : dossiers listés explicitement, fichiers par chemin.
    #[derive(Default, Clone, Debug)]
    pub struct FauxDistant {
        /// Listage rendu pour chaque dossier.
        pub listages: BTreeMap<String, Listing>,
        /// Contenu de chaque fichier.
        pub fichiers: BTreeMap<String, Vec<u8>>,
    }

    impl FauxDistant {
        /// Ajoute un dossier et son listage (nom, dossier ?), non tronqué.
        pub fn dossier(&mut self, chemin: &str, entrees: &[(&str, bool)]) {
            let entrees = entrees
                .iter()
                .map(|(nom, dossier)| Entree {
                    nom: (*nom).to_owned(),
                    dossier: *dossier,
                })
                .collect();
            self.listages.insert(
                chemin.to_owned(),
                Listing {
                    entrees,
                    tronque: false,
                },
            );
        }
    }

    // Faux sans E/S : les méthodes sont `async` parce que le trait `DistantFs` l'exige (`sync/command.sdd`).
    #[allow(clippy::unused_async_trait_impl)]
    impl DistantFs for FauxDistant {
        async fn list(&mut self, chemin: &str) -> Result<Listing, FsError> {
            self.listages
                .get(chemin)
                .cloned()
                .ok_or_else(|| FsError::Protocol {
                    op: "list",
                    message: format!("VNL-TLS-001: not found: {chemin}"),
                })
        }

        async fn read_raw(&mut self, chemin: &str) -> Result<Vec<u8>, FsError> {
            self.fichiers
                .get(chemin)
                .cloned()
                .ok_or_else(|| FsError::Protocol {
                    op: "read",
                    message: format!("VNL-TLS-001: not found: {chemin}"),
                })
        }

        async fn write(&mut self, chemin: &str, octets: &[u8]) -> Result<(), FsError> {
            self.fichiers.insert(chemin.to_owned(), octets.to_vec());
            self.referencer(chemin);
            Ok(())
        }

        async fn delete(&mut self, chemin: &str) -> Result<bool, FsError> {
            let existait = self.fichiers.remove(chemin).is_some();
            if existait {
                self.oublier(chemin);
            }
            Ok(existait)
        }
    }

    impl FauxDistant {
        /// Ajoute `chemin` au listage de chacun de ses dossiers parents, comme la sandbox le ferait.
        fn referencer(&mut self, chemin: &str) {
            let segments: Vec<&str> = chemin.split('/').collect();
            let dernier = segments.len().saturating_sub(1);
            for (i, nom) in segments.iter().enumerate() {
                let parent = segments.get(..i).map(|p| p.join("/")).unwrap_or_default();
                let listage = self.listages.entry(parent).or_insert_with(|| Listing {
                    entrees: Vec::new(),
                    tronque: false,
                });
                let dossier = i < dernier;
                if !listage.entrees.iter().any(|e| e.nom == *nom) {
                    listage.entrees.push(Entree {
                        nom: (*nom).to_owned(),
                        dossier,
                    });
                }
            }
        }

        /// Retire `chemin` du listage de son dossier parent.
        fn oublier(&mut self, chemin: &str) {
            let segments: Vec<&str> = chemin.split('/').collect();
            let parent = segments
                .get(..segments.len().saturating_sub(1))
                .map(|p| p.join("/"))
                .unwrap_or_default();
            let nom = segments.last().copied().unwrap_or_default();
            if let Some(listage) = self.listages.get_mut(&parent) {
                listage.entrees.retain(|e| e.nom != nom);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::essai::FauxDistant;
    use super::*;

    /// Scenario `chemin hostile` : un listage distant contenant `../x` est refusé, nommé.
    #[tokio::test]
    async fn chemin_hostile() {
        let mut faux = FauxDistant::default();
        faux.dossier(DOSSIER, &[("../x", false)]);
        let err = read_remote(&mut faux).await.expect_err("doit échouer");
        match err {
            TreeError::Path { chemin, .. } => assert!(chemin.contains("../x"), "chemin : {chemin}"),
            autre => panic!("obtenu : {autre:?}"),
        }
    }

    /// Scenario `fichier non texte ou trop gros` (cas du fichier non texte).
    #[test]
    fn fichier_non_texte_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("bin.dat"), [0xff_u8, 0xfe, 0x00]).expect("écriture");
        let err = read_local(dir.path()).expect_err("doit échouer");
        assert!(
            matches!(err, TreeError::NotText { ref chemin } if chemin.contains("bin.dat")),
            "obtenu : {err:?}"
        );
    }

    /// Scenario `fichier non texte ou trop gros` (cas du fichier trop gros, 300 Kio).
    #[test]
    fn fichier_trop_gros_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("gros.md"), vec![b'a'; 300 * 1024]).expect("écriture");
        let err = read_local(dir.path()).expect_err("doit échouer");
        assert!(
            matches!(err, TreeError::TooLarge { ref chemin, .. } if chemin.contains("gros.md")),
            "obtenu : {err:?}"
        );
    }

    /// Scenario `listage tronqué` : un listage marqué tronqué fait échouer la lecture.
    #[tokio::test]
    async fn listage_tronque() {
        let mut faux = FauxDistant::default();
        faux.dossier(DOSSIER, &[("a.md", false)]);
        if let Some(l) = faux.listages.get_mut(DOSSIER) {
            l.tronque = true;
        }
        let err = read_remote(&mut faux).await.expect_err("doit échouer");
        assert!(
            matches!(err, TreeError::Truncated { .. }),
            "obtenu : {err:?}"
        );
    }

    /// Scenario `arbre absent` : aucun `.vanyline/` (local ou distant) donne un arbre vide.
    #[tokio::test]
    async fn arbre_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let local = read_local(&dir.path().join(DOSSIER)).expect("arbre local absent");
        assert!(local.is_empty());
        let mut faux = FauxDistant::default();
        let distant = read_remote(&mut faux).await.expect("arbre distant absent");
        assert!(distant.is_empty());
    }

    /// Octets exacts (saut de ligne final compris) et empreinte SHA-256 sur ces octets (`AC-1`).
    #[test]
    fn lecture_locale_octets_exacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let racine = dir.path().join(DOSSIER);
        std::fs::create_dir_all(racine.join("agents")).expect("dossier");
        std::fs::write(racine.join("agents/a.md"), "x\n\ny\n").expect("écriture");
        let arbre = read_local(&racine).expect("lecture");
        let cle = RelPath::new("agents/a.md").expect("chemin");
        let entree = arbre.get(&cle).expect("présent");
        assert_eq!(entree.octets, b"x\n\ny\n");
        assert_eq!(entree.sha256, empreinte(b"x\n\ny\n"));
    }

    /// Un lien symbolique n'est jamais suivi : il est refusé, nommé (`sync/tree.sdd`, `Must not`).
    #[cfg(unix)]
    #[test]
    fn lien_symbolique_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let racine = dir.path().join(DOSSIER);
        std::fs::create_dir_all(&racine).expect("dossier");
        let cible = dir.path().join("hors.txt");
        std::fs::write(&cible, "hors").expect("cible");
        std::os::unix::fs::symlink(&cible, racine.join("lien.md")).expect("lien");
        let err = read_local(&racine).expect_err("doit échouer");
        assert!(
            matches!(err, TreeError::Path { ref chemin, .. } if chemin.contains("lien.md")),
            "obtenu : {err:?}"
        );
    }

    /// Chemins relatifs : composantes normales seulement.
    #[test]
    fn relpath_refuse_les_chemins_hostiles() {
        assert!(RelPath::new("agents/a.md").is_ok());
        for mauvais in ["", "/abs", "../x", "a/../b", "a//b", "./a", "a\\b", "a\0b"] {
            assert!(
                RelPath::new(mauvais).is_err(),
                "accepté à tort : {mauvais:?}"
            );
        }
        let base = RelPath::new("agents").expect("base");
        assert!(base.joindre("..").is_err());
        assert!(base.joindre("x/y").is_err());
        assert_eq!(base.joindre("a.md").expect("nom").as_str(), "agents/a.md");
    }

    /// Lecture distante récursive : sous-dossiers listés un par un, contenus lus en brut.
    #[tokio::test]
    async fn lecture_distante_recursive() {
        let mut faux = FauxDistant::default();
        faux.dossier(DOSSIER, &[("agents", true), ("config.yaml", false)]);
        faux.dossier(".vanyline/agents", &[("a.md", false)]);
        faux.fichiers
            .insert(".vanyline/config.yaml".to_owned(), b"k: v\n".to_vec());
        faux.fichiers
            .insert(".vanyline/agents/a.md".to_owned(), b"# a\n".to_vec());
        let arbre = read_remote(&mut faux).await.expect("lecture");
        let a = RelPath::new("agents/a.md").expect("chemin");
        assert_eq!(
            arbre.get(&a).map(|e| e.octets.clone()),
            Some(b"# a\n".to_vec())
        );
        let c = RelPath::new("config.yaml").expect("chemin");
        assert_eq!(arbre.get(&c).map(|e| e.sha256), Some(empreinte(b"k: v\n")));
    }
}
