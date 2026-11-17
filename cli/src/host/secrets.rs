//! Résolution des noms de secrets depuis `secrets/secrets.yaml` du niveau utilisateur. Contrat : `host/secrets.sdd`.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use zeroize::Zeroize as _;

use super::HostError;
use super::files;
use super::layers::Layers;

/// Sous-répertoire des secrets, sous le niveau utilisateur.
const SECRETS_DIR: &str = "secrets";
/// Fichier des secrets, dans le sous-répertoire.
const SECRETS_FILE: &str = "secrets.yaml";
/// Droits du fichier : privé à l'utilisateur.
const MODE_FICHIER: u32 = 0o600;
/// Droits du répertoire : privé à l'utilisateur.
const MODE_DOSSIER: u32 = 0o700;
/// Masque des bits de droits d'un fichier ou répertoire.
const MODE_BITS: u32 = 0o777;

/// Marqueur de droits trop larges.
const PERMS: &str = "VNL-CLI-HST-015";
/// Marqueur d'un nom de secret absent.
const UNKNOWN: &str = "VNL-CLI-HST-016";
/// Marqueur d'un fichier de secrets qui n'a pas la forme attendue.
const FORMAT: &str = "VNL-CLI-HST-017";
/// Marqueur d'une lecture des droits impossible.
const STAT: &str = "VNL-CLI-HST-018";
/// Marqueur d'une création du répertoire des secrets impossible.
const MKDIR: &str = "VNL-CLI-HST-019";
/// Marqueur d'une sérialisation impossible.
const ENCODE: &str = "VNL-CLI-HST-020";

/// Valeur d'un secret : opaque, effacée à la chute. Son `Debug` et son `Display` ne rendent que `***`.
pub struct SecretValue {
    valeur: String,
}

impl SecretValue {
    /// Enveloppe `valeur`.
    #[must_use]
    pub fn new(valeur: String) -> Self {
        Self { valeur }
    }

    /// La valeur brute, à n'appeler qu'au point d'usage (en-tête d'authentification).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.valeur
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.valeur.zeroize();
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl std::fmt::Display for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// Résolution des noms de secrets sur `<utilisateur>/secrets/secrets.yaml`. Ne crée rien à la construction.
#[derive(Debug, Clone)]
pub struct Secrets {
    dossier: PathBuf,
    fichier: PathBuf,
}

impl Secrets {
    /// Résolveur rangé sous le niveau utilisateur de `layers`.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        let dossier = layers.user().join(SECRETS_DIR);
        let fichier = dossier.join(SECRETS_FILE);
        Self { dossier, fichier }
    }

    /// Valeur du secret `nom`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Secret`] for unknown names or too-wide permissions, [`HostError::Format`] for an invalid
    /// file.
    pub fn resolve(&self, nom: &str) -> Result<SecretValue, HostError> {
        let Some(mut entrees) = self.read_entries()? else {
            return Err(unknown(nom));
        };
        entrees.remove(nom).ok_or_else(|| unknown(nom))
    }

    /// Écrit ou remplace le secret `nom` sans toucher aux autres entrées.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Secret`] when existing permissions are too wide, [`HostError::Format`] or
    /// [`HostError::Io`] for the other failures.
    pub fn set(&self, nom: &str, valeur: SecretValue) -> Result<(), HostError> {
        let mut entrees = self.read_entries()?.unwrap_or_default();
        self.create_directory()?;
        entrees.insert(nom.to_owned(), valeur);
        self.write_entries(&entrees)
    }

    /// Lit toutes les entrées, `None` si le fichier n'existe pas. Les droits sont contrôlés avant toute lecture.
    fn read_entries(&self) -> Result<Option<BTreeMap<String, SecretValue>>, HostError> {
        check_mode(&self.dossier, MODE_DOSSIER)?;
        check_mode(&self.fichier, MODE_FICHIER)?;
        let Some(texte) = files::read_text(&self.fichier)? else {
            return Ok(None);
        };
        parse(&self.fichier, &texte).map(Some)
    }

    /// Crée le répertoire des secrets en 0700 s'il est absent. Un répertoire existant n'est pas modifié.
    fn create_directory(&self) -> Result<(), HostError> {
        if mode_of(&self.dossier)?.is_some() {
            return Ok(());
        }
        if let Some(parent) = self.dossier.parent() {
            fs::create_dir_all(parent).map_err(|erreur| mkdir_error(&self.dossier, &erreur))?;
        }
        DirBuilder::new()
            .mode(MODE_DOSSIER)
            .create(&self.dossier)
            .map_err(|erreur| mkdir_error(&self.dossier, &erreur))
    }

    /// Écrit toutes les entrées de façon atomique en 0600, puis efface le texte sérialisé.
    fn write_entries(&self, entrees: &BTreeMap<String, SecretValue>) -> Result<(), HostError> {
        let vues: BTreeMap<&str, &str> = entrees
            .iter()
            .map(|(nom, valeur)| (nom.as_str(), valeur.expose()))
            .collect();
        let mut texte = yaml_serde::to_string(&vues).map_err(|_| HostError::Format {
            chemin: self.fichier.clone(),
            cause: format!("{ENCODE}: the secrets could not be serialized"),
        })?;
        let resultat = files::write_atomic(&self.fichier, texte.as_bytes(), MODE_FICHIER);
        texte.zeroize();
        resultat
    }
}

/// Erreur d'un nom absent du fichier (ou du fichier absent). Ne porte que le nom.
fn unknown(nom: &str) -> HostError {
    HostError::Secret {
        cause: format!("{UNKNOWN}: secret inconnu : {nom}"),
    }
}

/// Erreur de création du répertoire des secrets.
fn mkdir_error(chemin: &Path, erreur: &io::Error) -> HostError {
    HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{MKDIR}: {erreur}"),
    }
}

/// Mode (bits de droits) de `chemin`, `None` s'il n'existe pas.
fn mode_of(chemin: &Path) -> Result<Option<u32>, HostError> {
    match fs::metadata(chemin) {
        Ok(meta) => Ok(Some(meta.permissions().mode() & MODE_BITS)),
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(erreur) => Err(HostError::Io {
            chemin: chemin.to_path_buf(),
            cause: format!("{STAT}: {erreur}"),
        }),
    }
}

/// Refuse `chemin` s'il est plus ouvert que `attendu` (le chemin et les deux modes sont nommés, pas le contenu).
fn check_mode(chemin: &Path, attendu: u32) -> Result<(), HostError> {
    let Some(lu) = mode_of(chemin)? else {
        return Ok(());
    };
    if lu & !attendu != 0 {
        return Err(HostError::Secret {
            cause: format!(
                "{PERMS}: {} has mode {lu:o}, expected {attendu:o} or stricter",
                chemin.display()
            ),
        });
    }
    Ok(())
}

/// Lit le mappage plat « nom → chaîne ». Toute autre forme rend une erreur de format sans citer le contenu.
fn parse(chemin: &Path, texte: &str) -> Result<BTreeMap<String, SecretValue>, HostError> {
    let format = || HostError::Format {
        chemin: chemin.to_path_buf(),
        cause: format!("{FORMAT}: the secrets file must be a flat mapping of names to strings"),
    };
    if texte.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let brut: BTreeMap<String, yaml_serde::Value> =
        yaml_serde::from_str(texte).map_err(|_| format())?;
    let mut entrees = BTreeMap::new();
    for (nom, valeur) in brut {
        match valeur {
            yaml_serde::Value::String(texte) => {
                entrees.insert(nom, SecretValue::new(texte));
            }
            _ => return Err(format()),
        }
    }
    Ok(entrees)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use super::{SecretValue, Secrets};
    use crate::host::HostError;
    use crate::host::layers::Layers;

    /// Racine utilisateur fictive, `secrets/` en dessous.
    fn fixture(home: &Path) -> (Layers, PathBuf, PathBuf) {
        let layers = Layers::discover(home, home, None, |_| false);
        let dossier = home.join(".config").join("vanyline").join("secrets");
        let fichier = dossier.join("secrets.yaml");
        (layers, dossier, fichier)
    }

    fn mode(chemin: &Path) -> u32 {
        fs::metadata(chemin)
            .expect("the path must have metadata")
            .permissions()
            .mode()
            & 0o777
    }

    fn place(dossier: &Path, fichier: &Path, contenu: &str, mode_dossier: u32, mode_fichier: u32) {
        fs::create_dir_all(dossier).expect("the secrets directory must be created");
        fs::write(fichier, contenu).expect("the secrets file must be written");
        fs::set_permissions(dossier, fs::Permissions::from_mode(mode_dossier))
            .expect("the directory mode must be set");
        fs::set_permissions(fichier, fs::Permissions::from_mode(mode_fichier))
            .expect("the file mode must be set");
    }

    fn valeur(secrets: &Secrets, nom: &str) -> String {
        secrets
            .resolve(nom)
            .expect("the secret must resolve")
            .expose()
            .to_owned()
    }

    #[test]
    fn too_wide_file_mode_is_refused_without_reading() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        place(&dossier, &fichier, "cle: valeur-secrete\n", 0o700, 0o644);

        let resultat = Secrets::new(&layers).resolve("cle");

        match resultat {
            Err(HostError::Secret { cause }) => {
                assert!(
                    cause.contains("secrets.yaml"),
                    "the cause must name the path: {cause}"
                );
                assert!(
                    cause.contains("644"),
                    "the cause must give the mode read: {cause}"
                );
                assert!(
                    cause.contains("600"),
                    "the cause must give the expected mode: {cause}"
                );
                assert!(
                    !cause.contains("valeur-secrete"),
                    "the cause must not hold the value"
                );
            }
            autre => panic!("a file mode of 0644 must be refused as a secret error, got {autre:?}"),
        }
    }

    #[test]
    fn too_wide_directory_mode_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        place(&dossier, &fichier, "cle: valeur\n", 0o755, 0o600);

        let resultat = Secrets::new(&layers).resolve("cle");

        assert!(
            matches!(resultat, Err(HostError::Secret { ref cause }) if cause.contains("755")),
            "a directory mode of 0755 must be refused with the mode read, got {resultat:?}"
        );
    }

    #[test]
    fn secret_value_is_never_displayed() {
        let secret = SecretValue::new("valeur-tres-secrete".to_owned());

        assert_eq!(format!("{secret:?}"), "***", "Debug must hide the value");
        assert_eq!(format!("{secret}"), "***", "Display must hide the value");
        assert_eq!(
            secret.expose(),
            "valeur-tres-secrete",
            "expose must give the value"
        );
    }

    #[test]
    fn unknown_name_is_reported_with_the_name_only() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        place(&dossier, &fichier, "present: \"1\"\n", 0o700, 0o600);

        let resultat = Secrets::new(&layers).resolve("absent");

        match resultat {
            Err(HostError::Secret { cause }) => {
                assert!(
                    cause.contains("secret inconnu"),
                    "the cause must say unknown: {cause}"
                );
                assert!(
                    cause.contains("absent"),
                    "the cause must name the secret: {cause}"
                );
            }
            autre => panic!("an unknown name must be a secret error, got {autre:?}"),
        }
    }

    #[test]
    fn missing_file_makes_every_name_unknown() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, _, _) = fixture(home.path());

        let resultat = Secrets::new(&layers).resolve("nimporte");

        assert!(
            matches!(resultat, Err(HostError::Secret { ref cause }) if cause.contains("secret inconnu")),
            "a missing file must make the name unknown, got {resultat:?}"
        );
    }

    #[test]
    fn writing_then_resolving_keeps_other_entries_and_modes() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        let secrets = Secrets::new(&layers);

        secrets
            .set("premier", SecretValue::new("v1".to_owned()))
            .expect("writing into an absent file must succeed");

        assert_eq!(mode(&fichier), 0o600, "the secrets file must be 0600");
        assert_eq!(mode(&dossier), 0o700, "the secrets directory must be 0700");
        assert_eq!(
            valeur(&secrets, "premier"),
            "v1",
            "the written value must be read back"
        );

        secrets
            .set("second", SecretValue::new("v2".to_owned()))
            .expect("a second write must succeed");

        assert_eq!(
            valeur(&secrets, "premier"),
            "v1",
            "the first entry must be kept"
        );
        assert_eq!(
            valeur(&secrets, "second"),
            "v2",
            "the second entry must be readable"
        );
        assert_eq!(
            mode(&fichier),
            0o600,
            "the mode must stay 0600 after a rewrite"
        );
    }

    #[test]
    fn writing_refuses_a_directory_with_too_wide_mode() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        fs::create_dir_all(&dossier).expect("the secrets directory must be created");
        fs::set_permissions(&dossier, fs::Permissions::from_mode(0o755))
            .expect("the directory mode must be set");

        let resultat = Secrets::new(&layers).set("cle", SecretValue::new("v".to_owned()));

        assert!(
            matches!(resultat, Err(HostError::Secret { .. })),
            "writing into a 0755 directory must be refused, got {resultat:?}"
        );
        assert!(
            !fichier.exists(),
            "no file must be written into a too-wide directory"
        );
    }

    #[test]
    fn rotation_is_seen_by_the_next_resolution() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        place(&dossier, &fichier, "cle: ancienne\n", 0o700, 0o600);
        let secrets = Secrets::new(&layers);
        assert_eq!(
            valeur(&secrets, "cle"),
            "ancienne",
            "the first value must resolve"
        );

        fs::write(&fichier, "cle: nouvelle\n").expect("the rotated file must be written");

        assert_eq!(
            valeur(&secrets, "cle"),
            "nouvelle",
            "the rotated value must be seen"
        );
    }

    #[test]
    fn a_non_string_value_is_a_format_error_without_content() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, dossier, fichier) = fixture(home.path());
        place(&dossier, &fichier, "cle:\n  imbrique: 1\n", 0o700, 0o600);

        let resultat = Secrets::new(&layers).resolve("cle");

        assert!(
            matches!(resultat, Err(HostError::Format { ref chemin, .. }) if chemin == &fichier),
            "a non-string value must be a format error carrying the path, got {resultat:?}"
        );
    }
}
