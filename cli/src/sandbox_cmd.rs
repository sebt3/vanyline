use clap::Subcommand;

/// Parse `NAME=IMAGE` (ex. `rust=rust:slim-trixie`) en `Toolchain` avec un
/// env vide — le controller applique un preset d'environnement connu pour
/// les noms `rust`/`node` quand `env` est vide (cf. `resolve_toolchain_env`
/// cote controller), donc laisser `env` vide ici est le comportement voulu,
/// pas un oubli.
///
/// `split_once` prend le premier `=` : les images Docker n'utilisent pas `=`
/// dans leur reference, donc c'est l'unique `=` attendu et le comportement est
/// correct.
fn parse_toolchain(s: &str) -> Result<vanyline_crds::Toolchain, String> {
    let (name, image) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid --toolchain '{s}', expected NAME=IMAGE"))?;
    if name.is_empty() || image.is_empty() {
        return Err(format!("invalid --toolchain '{s}', expected NAME=IMAGE"));
    }
    Ok(vanyline_crds::Toolchain {
        name: name.to_string(),
        image: image.to_string(),
        env: Default::default(),
    })
}

#[derive(Subcommand)]
pub enum Commands {
    /// List K8s Sandboxes
    List,
    /// Show a single Sandbox
    Show { name: String },
    /// Create a Sandbox
    Create {
        name: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        branch: String,
        /// Repetable : --toolchain rust=rust:slim-trixie --toolchain node=node:trixie-slim
        #[arg(long = "toolchain", value_parser = parse_toolchain)]
        toolchains: Vec<vanyline_crds::Toolchain>,
        /// Image du serveur sandbox. Omis: defaut du controller (env `SANDBOX_IMAGE`).
        #[arg(long)]
        image: Option<String>,
    },
    /// Delete a Sandbox
    Delete { name: String },
    /// Stop a Sandbox (suspend the pod without deleting the resource)
    Stop { name: String },
    /// Start a previously stopped Sandbox
    Start { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_toolchain_valid() {
        let tc = parse_toolchain("rust=rust:slim-trixie").unwrap();
        assert_eq!(tc.name, "rust");
        assert_eq!(tc.image, "rust:slim-trixie");
        assert!(tc.env.is_empty());
    }

    #[test]
    fn parse_toolchain_missing_equals_errors() {
        let err = parse_toolchain("rust").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }

    #[test]
    fn parse_toolchain_empty_name_errors() {
        let err = parse_toolchain("=rust:slim").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }

    #[test]
    fn parse_toolchain_empty_image_errors() {
        let err = parse_toolchain("rust=").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }
}
