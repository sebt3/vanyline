//! Codes de sortie communs à `run`, `script` et aux commandes d'objets.
//! Contrat : `exit_code.sdd`. Pur, sans E/S.

use std::process::ExitCode;

use lymaille::RunStatus;

/// Issue d'une commande, avant sa projection en code de sortie (`exit_code.sdd`, `@Outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Le tour, le script ou la commande a réussi.
    Success,
    /// Échec du tour, du script ou de la commande.
    Failure,
    /// Usage invalide.
    Usage,
    /// Délai `-t` dépassé.
    Timeout,
    /// Annulation par l'utilisateur (Ctrl+C).
    Interrupted,
}

/// Code de sortie du processus pour une issue (`exit_code.sdd`, `@from_outcome`).
#[must_use]
pub fn from_outcome(outcome: Outcome) -> ExitCode {
    match outcome {
        Outcome::Success => ExitCode::SUCCESS,
        Outcome::Failure => ExitCode::from(1),
        Outcome::Usage => ExitCode::from(2),
        Outcome::Timeout => ExitCode::from(3),
        Outcome::Interrupted => ExitCode::from(130),
    }
}

/// Projection d'une annulation : l'échéance prime sur Ctrl+C, et une annulation sans cause connue est une interruption.
fn cancelled(deadline_hit: bool) -> Outcome {
    if deadline_hit {
        Outcome::Timeout
    } else {
        Outcome::Interrupted
    }
}

/// Issue d'un tour de lymaille, avec les causes d'annulation connues de l'appelant (`exit_code.sdd`, `@from_turn`).
///
/// L'échéance prime sur Ctrl+C quand les deux causes sont vraies.
#[must_use]
pub fn from_turn(outcome: lymaille::Outcome, deadline_hit: bool, user_cancelled: bool) -> Outcome {
    match outcome {
        lymaille::Outcome::Valid => Outcome::Success,
        lymaille::Outcome::Cancelled => {
            let _ = user_cancelled;
            cancelled(deadline_hit)
        }
        // `Failed`, et toute issue future (`Outcome` est `#[non_exhaustive]`), comptent comme un échec.
        _ => Outcome::Failure,
    }
}

/// Issue d'un script, à partir de son statut lymaille et des mêmes causes d'annulation (`exit_code.sdd`, `@from_script`).
#[must_use]
pub fn from_script(statut: RunStatus, deadline_hit: bool, user_cancelled: bool) -> Outcome {
    match statut {
        RunStatus::Ok => Outcome::Success,
        RunStatus::Cancelled => {
            let _ = user_cancelled;
            cancelled(deadline_hit)
        }
        // `Failed`, et tout statut futur (`RunStatus` est `#[non_exhaustive]`), comptent comme un échec.
        _ => Outcome::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lymaille::ScriptErrorClass;

    #[test]
    fn tableau_des_codes() {
        assert_eq!(from_outcome(Outcome::Success), ExitCode::from(0));
        assert_eq!(from_outcome(Outcome::Failure), ExitCode::from(1));
        assert_eq!(from_outcome(Outcome::Usage), ExitCode::from(2));
        assert_eq!(from_outcome(Outcome::Timeout), ExitCode::from(3));
        assert_eq!(from_outcome(Outcome::Interrupted), ExitCode::from(130));
    }

    #[test]
    fn annulation_par_echeance_ou_par_l_utilisateur() {
        assert_eq!(
            from_turn(lymaille::Outcome::Cancelled, true, false),
            Outcome::Timeout
        );
        assert_eq!(
            from_turn(lymaille::Outcome::Cancelled, false, true),
            Outcome::Interrupted
        );
        assert_eq!(
            from_turn(lymaille::Outcome::Cancelled, true, true),
            Outcome::Timeout
        );
    }

    #[test]
    fn cancelled_sans_cause_connue_est_interrompu() {
        assert_eq!(
            from_turn(lymaille::Outcome::Cancelled, false, false),
            Outcome::Interrupted
        );
    }

    #[test]
    fn issues_de_lymaille() {
        assert_eq!(
            from_turn(lymaille::Outcome::Valid, false, false),
            Outcome::Success
        );
        assert_eq!(
            from_turn(lymaille::Outcome::Failed, false, false),
            Outcome::Failure
        );
    }

    #[test]
    fn statut_de_script() {
        assert_eq!(from_script(RunStatus::Ok, false, false), Outcome::Success);
        assert_eq!(
            from_script(RunStatus::Failed(ScriptErrorClass::Syntax), false, false),
            Outcome::Failure
        );
        assert_eq!(
            from_script(RunStatus::Cancelled, true, false),
            Outcome::Timeout
        );
        assert_eq!(
            from_script(RunStatus::Cancelled, false, true),
            Outcome::Interrupted
        );
    }
}
