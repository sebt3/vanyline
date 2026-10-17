use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// List configured toolsets
    List,
}

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands as RootCommands};
    use clap::Parser;

    use super::Commands as ToolsetCmd;

    // Scenario: vanyline_toolsets_list_se_parse_en_commands_toolsets_list
    #[test]
    fn vanyline_toolsets_list_se_parse_en_commands_toolsets_list() {
        match Cli::try_parse_from(["vanyline", "toolsets", "list"])
            .ok()
            .and_then(|c| c.command)
        {
            Some(RootCommands::Toolsets(ToolsetCmd::List)) => {}
            _ => panic!("expected Toolsets(List)"),
        }
    }

    // Scenario: une_sous_commande_inconnue_de_toolsets_est_rejetee
    #[test]
    fn une_sous_commande_inconnue_de_toolsets_est_rejetee() {
        assert!(Cli::try_parse_from(["vanyline", "toolsets", "supprimer"]).is_err());
    }
}
