//! Pont entre la session lymaille et la boucle du TUI — contrat complet : `./sink.sdd`.
//!
//! [`TuiSink`] est l'`EventSink` du chat interactif : il transmet chaque
//! [`Event`] de la session à la boucle de `./mod.rs` par un canal
//! `tokio::sync::mpsc` NON BORNÉ, sans jamais bloquer le tour, sans filtrer
//! ni transformer. La décision d'affichage appartient à `./state.rs`.

use lymaille::{Event, EventSink};
use tokio::sync::mpsc;

/// Sink du TUI : canal non borné vers la boucle d'événements.
#[derive(Debug, Clone)]
pub struct TuiSink {
    tx: mpsc::UnboundedSender<Event>,
}

impl TuiSink {
    /// Crée le sink et son récepteur (transporte directement les [`Event`] de lymaille).
    #[must_use]
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl EventSink for TuiSink {
    // Envoi vers un canal non borné : jamais de blocage, jamais de panique ;
    // un récepteur fermé (TUI quittée) est ignoré silencieusement. Aucun
    // filtrage : la décision d'affichage appartient à ./state.rs.
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lymaille::{EventKind, Redactor, ScriptInput, ScriptRecord, SessionId, Timestamp, TurnId};
    use std::time::Duration;

    /// Événement de la session `s` portant `kind`, daté à l'instant 0.
    fn evenement(kind: EventKind) -> Event {
        Event::new(
            SessionId::new("s").expect("un identifiant non vide"),
            Timestamp::from_millis(0),
            kind,
        )
    }

    /// Scenario: `ordre preserve` — mille fragments émis dans l'ordre arrivent dans le même ordre, aucun perdu.
    #[test]
    fn ordre_preserve() {
        let (sink, mut rx) = TuiSink::channel();
        let emis: Vec<Event> = (0..1000)
            .map(|i| {
                evenement(EventKind::TextDelta {
                    turn: TurnId::new(1),
                    text: format!("j{i}"),
                })
            })
            .collect();
        for ev in &emis {
            sink.emit(ev.clone());
        }
        let mut recus = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            recus.push(ev);
        }
        assert_eq!(recus.len(), 1000, "aucun événement perdu");
        assert_eq!(recus, emis, "même ordre");
    }

    /// Scenario: `recepteur ferme` — récepteur supprimé : `emit` ne panique ni ne bloque.
    #[tokio::test]
    async fn recepteur_ferme() {
        let (sink, rx) = TuiSink::channel();
        drop(rx);
        let retour = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(move || {
                sink.emit(evenement(EventKind::TodoChanged));
            }),
        )
        .await;
        assert!(retour.is_ok(), "emit vers un récepteur fermé ne bloque pas");
    }

    /// Scenario: `tous les evenements transmis` — un `TextDelta`, un `Script`, un `TokenUsage` et un `McpReconnect`
    /// arrivent tous (aucun filtrage).
    #[test]
    fn tous_les_evenements_transmis() {
        let (sink, mut rx) = TuiSink::channel();
        let script = ScriptRecord::new(
            &Redactor::new(),
            10,
            ScriptInput::new("agent-a", "print(1)", "1", Duration::from_millis(4)),
        );
        sink.emit(evenement(EventKind::TextDelta {
            turn: TurnId::new(1),
            text: "bonjour".into(),
        }));
        sink.emit(evenement(EventKind::Script(script)));
        sink.emit(evenement(EventKind::TokenUsage {
            prompt: 1,
            completion: 2,
        }));
        sink.emit(evenement(EventKind::McpReconnect {
            role: "monitoring".into(),
            attempt: 1,
            delay_ms: 500,
        }));
        let mut recus = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            recus.push(ev);
        }
        assert_eq!(recus.len(), 4, "les quatre événements arrivent: {recus:?}");
    }
}
