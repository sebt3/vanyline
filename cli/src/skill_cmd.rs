use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// List configured skills
    List,
}

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands as RootCommands};
    use clap::Parser;

    use super::Commands as SkillCmd;

    fn skills(args: &[&str]) -> Option<SkillCmd> {
        match Cli::try_parse_from(args).ok()?.command {
            Some(RootCommands::Skills(cmd)) => Some(cmd),
            _ => None,
        }
    }

    // Scenario: skills_list_parse
    #[test]
    fn skills_list_parse() {
        assert!(matches!(
            skills(&["vanyline", "skills", "list"]),
            Some(SkillCmd::List)
        ));
    }

    // Scenario: skills_sans_sous_commande_rejete
    #[test]
    fn skills_sans_sous_commande_rejete() {
        assert!(Cli::try_parse_from(["vanyline", "skills"]).is_err());
    }

    // Scenario: skills_sous_commande_inconnue_rejetee
    #[test]
    fn skills_sous_commande_inconnue_rejetee() {
        assert!(Cli::try_parse_from(["vanyline", "skills", "show"]).is_err());
    }

    // Scenario: skills_list_refuse_arguments
    #[test]
    fn skills_list_refuse_arguments() {
        assert!(Cli::try_parse_from(["vanyline", "skills", "list", "extra"]).is_err());
    }
}
