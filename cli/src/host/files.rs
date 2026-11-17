//! Lecture et écriture atomique de fichiers de l'hôte. Contrat : `host/files.sdd`.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::HostError;

/// Préfixe des fichiers temporaires de l'écriture atomique, ignorés par le listage.
const TMP_PREFIX: &str = ".tmp-";

/// Marqueur d'une lecture impossible.
const READ: &str = "VNL-CLI-HST-001";
/// Marqueur d'une écriture impossible.
const WRITE: &str = "VNL-CLI-HST-002";
/// Marqueur d'un listage de répertoire impossible.
const LIST: &str = "VNL-CLI-HST-003";
/// Marqueur d'une suppression impossible.
const REMOVE: &str = "VNL-CLI-HST-004";
/// Marqueur d'un contenu qui n'est pas de l'UTF-8.
const UTF8: &str = "VNL-CLI-HST-005";

/// Numéro unique des fichiers temporaires, pour qu'aucun ne soit écrasé.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Vrai si `nom` est exactement un composant de chemin normal : ni séparateur, ni `.`, ni `..`. Sert à refuser un
/// identifiant qui sortirait de son répertoire.
#[must_use]
pub fn est_un_nom_de_fichier(nom: &str) -> bool {
    let mut composants = Path::new(nom).components();
    matches!(
        (composants.next(), composants.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// Erreur d'E/S sur `chemin`, avec son marqueur.
fn io_error(chemin: &Path, marqueur: &str, erreur: &io::Error) -> HostError {
    HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{marqueur}: {erreur}"),
    }
}

/// Écrit `octets` dans `chemin` de façon atomique, avec les droits `mode` (`0o600` ou `0o644`). Crée les répertoires
/// parents.
///
/// # Errors
///
/// Returns [`HostError::Io`] when the file cannot be written.
pub fn write_atomic(chemin: &Path, octets: &[u8], mode: u32) -> Result<(), HostError> {
    write_atomic_with(chemin, octets, mode, |from, to| fs::rename(from, to))
}

/// Lit `chemin` en UTF-8. `None` si le fichier n'existe pas.
///
/// # Errors
///
/// Returns [`HostError::Io`] or [`HostError::Format`].
pub fn read_text(chemin: &Path) -> Result<Option<String>, HostError> {
    match fs::read(chemin) {
        Ok(octets) => String::from_utf8(octets)
            .map(Some)
            .map_err(|_| HostError::Format {
                chemin: chemin.to_path_buf(),
                cause: format!("{UTF8}: the file is not valid UTF-8"),
            }),
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(erreur) => Err(io_error(chemin, READ, &erreur)),
    }
}

/// Liste les fichiers réguliers de `repertoire` dont l'extension (sans point) est `extension`, triés par nom. Les
/// liens symboliques ne sont pas suivis ; les fichiers `.tmp-*` sont ignorés. Une entrée illisible rend un `Err` à sa
/// place, les autres restent listées.
#[must_use]
pub fn list_files(repertoire: &Path, extension: &str) -> Vec<Result<PathBuf, HostError>> {
    let lecture = match fs::read_dir(repertoire) {
        Ok(lecture) => lecture,
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(erreur) => return vec![Err(io_error(repertoire, LIST, &erreur))],
    };
    let mut fichiers: Vec<PathBuf> = Vec::new();
    let mut erreurs: Vec<Result<PathBuf, HostError>> = Vec::new();
    for entree in lecture {
        let entree = match entree {
            Ok(entree) => entree,
            Err(erreur) => {
                erreurs.push(Err(io_error(repertoire, LIST, &erreur)));
                continue;
            }
        };
        let chemin = entree.path();
        if entree.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
            continue;
        }
        match entree.file_type() {
            Ok(type_fichier)
                if type_fichier.is_file() && chemin.extension() == Some(OsStr::new(extension)) =>
            {
                fichiers.push(chemin);
            }
            Ok(_) => {}
            Err(erreur) => erreurs.push(Err(io_error(&chemin, LIST, &erreur))),
        }
    }
    fichiers.sort();
    fichiers.into_iter().map(Ok).chain(erreurs).collect()
}

/// Supprime `chemin`. `true` si le fichier existait.
///
/// # Errors
///
/// Returns [`HostError::Io`] when the removal fails.
pub fn remove_file(chemin: &Path) -> Result<bool, HostError> {
    match fs::remove_file(chemin) {
        Ok(()) => Ok(true),
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(erreur) => Err(io_error(chemin, REMOVE, &erreur)),
    }
}

/// Écriture atomique avec renommage injecté : le renommage final est un paramètre pour tester son échec.
pub(super) fn write_atomic_with<R>(
    chemin: &Path,
    octets: &[u8],
    mode: u32,
    rename: R,
) -> Result<(), HostError>
where
    R: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let dir = chemin
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(dir) = dir {
        fs::create_dir_all(dir).map_err(|erreur| io_error(chemin, WRITE, &erreur))?;
    }
    let tmp = temporary_path(chemin)?;
    let resultat = write_temporary(&tmp, octets, mode)
        .and_then(|()| rename(&tmp, chemin))
        .map_err(|erreur| io_error(chemin, WRITE, &erreur));
    if resultat.is_err() {
        // Nettoyage best-effort : l'échec d'origine est déjà rendu à l'appelant.
        fs::remove_file(&tmp).ok();
    }
    resultat
}

/// Chemin d'un fichier temporaire, dans le même répertoire que la cible (renommage atomique).
fn temporary_path(chemin: &Path) -> Result<PathBuf, HostError> {
    let Some(nom) = chemin.file_name() else {
        return Err(HostError::Io {
            chemin: chemin.to_path_buf(),
            cause: format!("{WRITE}: the target has no file name"),
        });
    };
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nom_tmp = format!(
        "{TMP_PREFIX}{}-{}-{sequence}",
        nom.to_string_lossy(),
        std::process::id()
    );
    Ok(chemin.with_file_name(nom_tmp))
}

/// Crée `tmp` (refusant tout fichier existant), y écrit `octets`, le synchronise et fixe son mode.
fn write_temporary(tmp: &Path, octets: &[u8], mode: u32) -> io::Result<()> {
    let mut fichier = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(tmp)?;
    fichier.write_all(octets)?;
    fichier.sync_all()?;
    fs::set_permissions(tmp, fs::Permissions::from_mode(mode))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use super::{HostError, list_files, read_text, remove_file, write_atomic, write_atomic_with};

    /// Renommage qui échoue toujours, comme un disque plein ou un arrêt au milieu de l'écriture.
    fn failing_rename(_from: &Path, _to: &Path) -> io::Result<()> {
        Err(io::Error::other("injected rename failure"))
    }

    #[test]
    fn atomic_write_keeps_previous_content_when_rename_fails() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        let cible = dir.path().join("etat.json");
        fs::write(&cible, b"ancien").expect("the fixture file must be written");

        let resultat = write_atomic_with(&cible, b"nouveau", 0o600, failing_rename);

        assert!(
            resultat.is_err(),
            "a failed rename must be reported to the caller, not swallowed"
        );
        let contenu = fs::read_to_string(&cible).expect("the target must still exist");
        assert_eq!(
            contenu, "ancien",
            "an interrupted write must leave the previous content of the target intact"
        );
    }

    #[test]
    fn atomic_write_replaces_content_creates_parents_and_sets_mode() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        let cible = dir.path().join("a").join("b").join("etat.json");

        write_atomic(&cible, b"contenu", 0o600)
            .expect("writing into a new directory tree must succeed");
        assert_eq!(
            fs::read_to_string(&cible).expect("the written file must be readable"),
            "contenu",
            "the target must hold the written bytes"
        );
        let mode = fs::metadata(&cible)
            .expect("the written file must have metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the requested mode must be applied to the written file"
        );

        write_atomic(&cible, b"remplace", 0o600)
            .expect("overwriting an existing target must succeed");
        assert_eq!(
            fs::read_to_string(&cible).expect("the overwritten file must be readable"),
            "remplace",
            "a second write must replace the content"
        );
    }

    #[test]
    fn reading_a_missing_file_gives_none() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        let absent = dir.path().join("absent.yaml");

        let lu = read_text(&absent);

        assert!(
            matches!(lu, Ok(None)),
            "a missing file must read as None, without error"
        );
    }

    #[test]
    fn an_unreadable_file_does_not_stop_the_other_reads() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        fs::write(dir.path().join("a.yaml"), "un: 1\n").expect("fixture a must be written");
        fs::write(dir.path().join("b.yaml"), [0xff_u8, 0xfe, 0x00])
            .expect("fixture b must be written");
        fs::write(dir.path().join("c.yaml"), "trois: 3\n").expect("fixture c must be written");

        let fichiers: Vec<_> = list_files(dir.path(), "yaml").into_iter().collect();
        assert_eq!(fichiers.len(), 3, "the three files must all be listed");
        let lus: Vec<_> = fichiers
            .iter()
            .map(|entree| match entree {
                Ok(chemin) => read_text(chemin),
                Err(_) => Err(HostError::Invalid {
                    cause: "unexpected listing error".to_owned(),
                }),
            })
            .collect();

        assert!(
            matches!(lus.first(), Some(Ok(Some(_)))),
            "the first readable file must be read"
        );
        assert!(
            matches!(lus.get(2), Some(Ok(Some(_)))),
            "the readable file after the broken one must still be read"
        );
        let casse = matches!(
            lus.get(1),
            Some(Err(HostError::Format { chemin, .. })) if chemin.ends_with("b.yaml")
        );
        assert!(
            casse,
            "the broken file must yield a format error carrying its path, got {:?}",
            lus.get(1)
        );
    }

    #[test]
    fn temporary_files_are_ignored_by_listing() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        fs::write(dir.path().join(".tmp-x.yaml"), "orphelin: true\n")
            .expect("the orphan must be written");
        fs::write(dir.path().join("vrai.yaml"), "ok: true\n")
            .expect("the real file must be written");

        let noms: Vec<String> = list_files(dir.path(), "yaml")
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|chemin| {
                chemin
                    .file_name()
                    .map(|nom| nom.to_string_lossy().into_owned())
            })
            .collect();

        assert_eq!(
            noms,
            vec!["vrai.yaml".to_owned()],
            "a temporary orphan must not appear in the listing"
        );
    }

    #[test]
    fn a_missing_directory_lists_nothing_and_removal_reports_existence() {
        let dir = tempfile::tempdir().expect("a temporary directory must be created");
        let absent = dir.path().join("absent");
        assert!(
            list_files(&absent, "yaml").is_empty(),
            "a missing directory must list nothing, without error"
        );

        let fichier = dir.path().join("a.yaml");
        fs::write(&fichier, "a: 1\n").expect("the fixture must be written");
        assert!(
            matches!(remove_file(&fichier), Ok(true)),
            "removing an existing file must report true"
        );
        assert!(
            matches!(remove_file(&fichier), Ok(false)),
            "removing a missing file must report false"
        );
    }
}
