//! Pont entre l'agent et la boucle du TUI — contrat complet : `./sink.sdd`.
//!
//! [`TuiSink`] est l'`EventSink` du chat interactif : il transmet chaque
//! [`ChatEvent`] à la boucle de `./mod.rs` par un canal `tokio::sync::mpsc`
//! NON BORNÉ, sans jamais bloquer le tour, sans filtrer ni transformer. Il
//! remplace `StdoutSink` pour le mode interactif ; le one-shot garde
//! `StdoutSink` et `JsonSink`.

use async_trait::async_trait;
use tokio::sync::mpsc;
use vanyline_lib::event::{ChatEvent, EventSink};

/// Sink du TUI : canal non borné vers la boucle d'événements.
#[derive(Debug, Clone)]
pub struct TuiSink {
    tx: mpsc::UnboundedSender<ChatEvent>,
}

impl TuiSink {
    /// Crée le sink et son récepteur (transporte directement les
    /// [`ChatEvent`], aucun type intermédiaire).
    #[must_use]
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ChatEvent>) {
        // Canal non borné réel : les ChatEvent vont directement à la boucle.
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

#[async_trait]
impl EventSink for TuiSink {
    // Envoi vers un canal non borné : jamais de blocage, jamais de panique ;
    // un récepteur fermé (TUI quittée) est ignoré silencieusement. Aucun
    // filtrage : la décision d'affichage appartient à ./state.rs.
    async fn emit(&self, event: ChatEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: `ordre_preserve` — mille événements émis dans l'ordre
    /// arrivent dans le même ordre, aucun perdu.
    #[tokio::test]
    async fn ordre_preserve() {
        let (sink, mut rx) = TuiSink::channel();
        let emitted: Vec<ChatEvent> = (0..1000)
            .map(|i| ChatEvent::Token {
                content: format!("j{i}"),
            })
            .collect();
        for ev in &emitted {
            sink.emit(ev.clone()).await;
        }
        let mut got = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            got.push(ev);
        }
        assert_eq!(got.len(), 1000, "aucun événement perdu");
        assert_eq!(got, emitted, "même ordre");
    }

    /// Scenario: `recepteur_ferme` — récepteur supprimé : `emit` ne panique
    /// ni ne bloque.
    #[tokio::test]
    async fn recepteur_ferme() {
        let (sink, rx) = TuiSink::channel();
        drop(rx);
        // timeout garde-fou : sans emit non bloquant, ce test pendrait.
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sink.emit(ChatEvent::Done),
        )
        .await;
        assert!(r.is_ok(), "emit vers un récepteur fermé ne bloque pas");
    }

    /// Scenario: `tous_les_evenements_transmis` — `ReasoningDelta`, `ToolResult`,
    /// `Usage` et `ToolUnavailable` arrivent tous (aucun filtrage, contrairement
    /// à `StdoutSink`).
    #[tokio::test]
    async fn tous_les_evenements_transmis() {
        let (sink, mut rx) = TuiSink::channel();
        sink.emit(ChatEvent::ReasoningDelta {
            content: "réfléchir".into(),
        })
        .await;
        sink.emit(ChatEvent::ToolResult {
            id: "t".into(),
            name: "n".into(),
            result: "r".into(),
            is_error: false,
        })
        .await;
        sink.emit(ChatEvent::Usage {
            input_tokens: 1,
            output_tokens: 2,
        })
        .await;
        sink.emit(ChatEvent::ToolUnavailable {
            server: "s".into(),
            reason: "hors ligne".into(),
        })
        .await;
        let mut got = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            got.push(ev);
        }
        assert_eq!(got.len(), 4, "les quatre événements arrivent: {got:?}");
    }
}
