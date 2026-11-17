//! Enveloppe des événements d'interface publiés par `app` dans les sessions lymaille —
//! contrat : `./events.sdd`. Notification pure : rien ne survit au rechargement, tout ce qui
//! doit persister appartient à la Solution qui le possède (`FR-9`, `CON-4`).

use miryad_core::lymaille::EventHub;
use serde::Serialize;

/// Enveloppe des événements d'interface publiés par l'app (`FR-8`) : le seul type
/// d'événement d'interface de vanyline (`Must` de `./events.sdd`). Le contenu des `kind`
/// appartient aux Solutions `app/ui-actions` et `app/dashboards`, pas ici ; l'app transporte
/// le `payload`, ne le lit pas (`Must not`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub(crate) struct HostEvent {
    /// Type d'interface de l'événement (non vide — la limite est posée par miryad-core).
    pub kind: String,
    /// Charge opaque pour l'app : `any` côté TypeScript (même overriding que
    /// `lib/src/event.rs` — sans lui ts-rs génère un `JsonValue` importé, non exporté).
    #[cfg_attr(feature = "ts-rs", ts(type = "any"))]
    pub payload: serde_json::Value,
}

/// Refus souple d'une publication par le wrapper [`publish_host`] : délibérément PAS une erreur
/// de session (ni `LymailleError` ni `HostError`) — un événement d'interface est une notification
/// pure et son refus ne doit jamais interrompre la session (`Handles` de `./events.sdd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HostPublishRefused;

/// Publie un événement d'hôte sur le canal de la session `session` (fine-wrap d'
/// [`EventHub::publish_host`] de miryad-core) : la validation de charge (kind non vide, payload
/// sérialisé ≤ `HOST_PAYLOAD_MAX`) est celle de miryad-core, non réécrite (`Must`).
///
/// Sur refus (`MRD-LYMAILLE-004`), journalise en `warn` sous l'identifiant `VNL-APP-SES-108` —
/// suite de la série `VNL-APP-SES-101..107` de `host.rs` — et rend [`HostPublishRefused`] ;
/// l'appelant ne doit pas le propager comme erreur de session.
///
/// # Errors
///
/// [`HostPublishRefused`] : la crate hôte a refusé la charge ; rien n'a été diffusé.
pub(crate) fn publish_host(
    hub: &EventHub,
    session: &lymaille::SessionId,
    kind: &str,
    payload: serde_json::Value,
) -> Result<(), HostPublishRefused> {
    hub.publish_host(session, kind, payload).map_err(|erreur| {
        tracing::warn!(
            "VNL-APP-SES-108: publication d'événement d'interface refusée par l'hôte lymaille (kind {kind:?}) : {erreur}"
        );
        HostPublishRefused
    })
}

#[cfg(test)]
mod tests {
    use super::{HostEvent, HostPublishRefused, publish_host};
    use miryad_core::lymaille::interactive::hub::{HOST_PAYLOAD_MAX, Subscription};
    use miryad_core::lymaille::{EventHub, LymailleError, SessionEventKind};
    use serde_json::json;
    use std::time::Duration;

    fn session(id: &str) -> lymaille::SessionId {
        lymaille::SessionId::new(id).expect("un texte non vide fait un identifiant de session")
    }

    /// Rien n'arrive à l'abonné dans la fenêtre observée (aucun événement partiel diffusé).
    async fn reste_silencieux(sub: &mut Subscription) -> bool {
        tokio::time::timeout(Duration::from_millis(50), sub.recv())
            .await
            .is_err()
    }

    // ── Scenario « charge refusée sans casse » (`AC-6`) ──

    /// Le refus est celui de miryad-core, non réécrit : `InvalidDefinition` (`MRD-LYMAILLE-004`)
    /// sur payload trop lourd comme sur `kind` vide, rien de diffusé (`Must` de `events.sdd`).
    #[tokio::test(start_paused = true)]
    async fn charge_refusee_par_le_hub_sans_diffusion() {
        let hub = EventHub::new(None, 16);
        hub.register(&session("s1"), "alice");
        let mut sub = hub.subscribe("alice", &session("s1"));

        // payload sérialisé > 65 536 octets
        let big = "x".repeat(HOST_PAYLOAD_MAX);
        let erreur = hub
            .publish_host(&session("s1"), "dashboard.show", json!({ "data": big }))
            .expect_err("un payload au-delà de la limite doit être refusé");
        assert!(
            matches!(erreur, LymailleError::InvalidDefinition(ref t) if t == "host event invalid"),
            "refus réécrit : {erreur:?}"
        );

        // kind vide : même refus, même texte
        let erreur = hub
            .publish_host(&session("s1"), "", json!({}))
            .expect_err("un kind vide doit être refusé");
        assert!(
            matches!(erreur, LymailleError::InvalidDefinition(ref t) if t == "host event invalid"),
            "refus réécrit : {erreur:?}"
        );

        assert!(
            reste_silencieux(&mut sub).await,
            "rien ne doit être diffusé sur un refus"
        );
    }

    /// Le wrapper de l'app rend un refus souple (jamais une erreur de session : pas de
    /// `LymailleError`/`HostError` sous son type), ne diffuse rien, et journalise le refus en
    /// `warn` sous l'identifiant `VNL-APP-SES-108` (`Handles` de `events.sdd`).
    #[tokio::test(start_paused = true)]
    async fn wrapper_refus_souple_journalise_sans_casser_la_session() {
        let hub = EventHub::new(None, 16);
        hub.register(&session("s1"), "alice");
        let mut sub = hub.subscribe("alice", &session("s1"));

        let (refus, journaux) = super::test_support::capture_logs(|| {
            publish_host(
                &hub,
                &session("s1"),
                "dashboard.show",
                json!({ "data": "x".repeat(HOST_PAYLOAD_MAX) }),
            )
        });
        assert!(
            matches!(refus, Err(HostPublishRefused)),
            "attendu un refus souple HostPublishRefused, obtenu : {refus:?}"
        );
        assert!(
            journaux
                .iter()
                .any(|l| l.contains("VNL-APP-SES-108") && l.contains("dashboard.show")),
            "le refus doit être journalisé en warn avec l'identifiant propre : {journaux:?}"
        );
        assert!(
            reste_silencieux(&mut sub).await,
            "aucun événement partiel ne doit être diffusé"
        );
    }

    /// Le `Must` « la limite est celle de miryad-core — vanyline ne revalide pas » : une charge
    /// juste sous la limite passe par le wrapper sans trimming ni réécriture.
    #[tokio::test(start_paused = true)]
    async fn charge_juste_sous_la_limite_passe() {
        let hub = EventHub::new(None, 16);
        hub.register(&session("s1"), "alice");
        let mut sub = hub.subscribe("alice", &session("s1"));
        // {"d":"…"} : la forme sérialisée fait exactement HOST_PAYLOAD_MAX octets.
        let filled = "x".repeat(HOST_PAYLOAD_MAX - "{\"d\":\"\"}".len());
        publish_host(
            &hub,
            &session("s1"),
            "dashboard.show",
            json!({ "d": filled }),
        )
        .expect("une charge à la limite exactement est valide");
        let recu = sub.recv().await.expect("l'abonné reçoit");
        assert!(matches!(
            recu.kind,
            SessionEventKind::Host { ref kind, .. } if kind == "dashboard.show"
        ));
    }

    // ── `Must not` « Interpréter un payload dans app » ──

    /// Publication valide par le wrapper : l'abonné reçoit le `Host { kind, payload }` identique.
    #[tokio::test(start_paused = true)]
    async fn payload_traverse_sans_reecriture() {
        let hub = EventHub::new(None, 16);
        hub.register(&session("s1"), "alice");
        let mut sub = hub.subscribe("alice", &session("s1"));
        let payload = json!({ "w": "chart", "n": [1, 2], "nim": { "ok": true } });
        publish_host(&hub, &session("s1"), "dashboard.show", payload.clone())
            .expect("une publication valide réussit");
        let recu = sub.recv().await.expect("l'abonné reçoit");
        assert_eq!(
            recu.kind,
            SessionEventKind::Host {
                kind: "dashboard.show".to_owned(),
                payload,
            }
        );
    }

    /// L'enveloppe Rust sérialise `{ kind, payload }` — la même forme que le type exporté.
    #[test]
    fn hote_event_serialize_kind_et_payload() {
        let evt = HostEvent {
            kind: "dashboard.show".to_owned(),
            payload: json!({ "w": "chart" }),
        };
        assert_eq!(
            serde_json::to_value(&evt).expect("l'enveloppe est sérialisable"),
            json!({ "kind": "dashboard.show", "payload": { "w": "chart" } })
        );
    }

    // ── Scenario « export TypeScript vérifié » (`AC-6`) ──

    /// Régénère `packages/protocol/src/generated/host-event.ts` ; fichier commité, le job CI
    /// `tsrs` régénère et vérifie `git diff --exit-code`.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_host_event_ts() {
        use ts_rs::{Config, TS};
        let cfg = Config::from_env();
        let evt = <HostEvent as TS>::export_to_string(&cfg).expect("HostEvent exportable");
        let out = format!(
            "// Generated by vanyline-app (ts-rs) from `HostEvent`.\n\
             // DO NOT EDIT — regenerate: cargo test -p vanyline-app --features ts-rs\n\n\
             {evt}\n"
        );
        let target = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/protocol/src/generated/host-event.ts"
        );
        std::fs::write(target, out).expect("l'export s'écrit");
        assert!(evt.contains("export type HostEvent"), "{evt}");
        assert!(evt.contains("payload: any"), "{evt}");
    }

    // ── Scenario « export TypeScript de la configuration lymaille » (#72) ──

    /// Régénère `packages/protocol/src/generated/lymaille/` par l'export de miryad-core ; les
    /// fichiers sont commités, le job CI `tsrs` vérifie `git diff --exit-code`.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_lymaille_types_ts() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../packages/protocol/src/generated/lymaille");
        let ecrits = miryad_core::lymaille::interactive::event::export_typescript(&dir)
            .expect("l'export de miryad-core s'écrit");
        assert!(!ecrits.is_empty());
        for attendu in ["agent.ts", "AgentDefinition.ts", "SessionEvent.ts"] {
            assert!(
                ecrits.iter().any(|p| p.ends_with(attendu)),
                "{attendu} absent de l'export"
            );
        }
    }
}

/// Support de test (aucun contrat) : capture des messages `tracing` émis pendant une
/// fermeture — les identifiants journalisés sont un contrat de `Handles`, pas une API.
#[cfg(test)]
mod test_support {
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    struct BufWriter(Buf);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .0
                .lock()
                .expect("tampon de capture jamais corrompu")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl tracing_subscriber::fmt::MakeWriter<'_> for Buf {
        type Writer = BufWriter;
        fn make_writer(&self) -> Self::Writer {
            BufWriter(self.clone())
        }
    }

    /// Exécute `f` sous un subscriber `fmt` mémoire (niveau `warn`), rend sa sortie et les
    /// lignes journalisées.
    pub(super) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        let sortie = tracing::subscriber::with_default(subscriber, f);
        let octets = {
            let mut garde = buf.0.lock().expect("tampon de capture jamais corrompu");
            std::mem::take(&mut *garde)
        };
        let lignes = String::from_utf8_lossy(&octets)
            .lines()
            .map(str::to_owned)
            .collect();
        (sortie, lignes)
    }
}
