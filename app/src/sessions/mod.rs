//! Sessions lymaille de l'app : hébergement des sessions par miryad-core — contrat :
//! `mod.sdd`. L'hôte vit dans ./host.rs, le catalogue dans ./catalog.rs, les événements dans
//! ./events.rs ; ce fichier porte le MONTAGE : l'unique hôte fourni à miryad-core, le
//! `SessionManager` et les routes de conversation sous `/api/v1/lymaille`, et le refus fermé
//! sans hôte (`FR-1`).

pub(crate) mod catalog;
#[allow(dead_code)]
// `publish_host`/`HostEvent` : aucun publieur branché avant la Solution `app/ui-actions`
// (`./events.sdd` `Tasks`) — le montage B9 branche l'hôte et le catalogue, pas les
// événements — tooling.sdd
mod events;
mod host;

use std::sync::Arc;

use axum::Router;
use sea_orm::DatabaseConnection;

use lymaille::TurnGate;
use miryad_core::lymaille::EnvSecretResolver;
use miryad_core::lymaille::EventHub;
use miryad_core::lymaille::SharedHost;
use miryad_core::lymaille::conversations_router;
use miryad_core::lymaille::interactive::assemble::AssembleDeps;
use miryad_core::lymaille::interactive::manager::{Limits, ManagerDeps, SessionManager};
use miryad_core::lymaille::resource_module::ResourceRegistry;

use crate::AppState;
use crate::error::AppError;
use host::K8sHostGateway;
use host::VnlLymailleHost;

/// File par session du hub d'événements — le défaut conseillé par `EventHub::new` de
/// miryad-core (borné à `[16, 4096]` par la crate).
const CAPACITE_HUB: usize = 256;

/// L'unique implémentation de `LymailleHost` de l'app (`mod.sdd` `Must`, `FR-1`) : la
/// connexion de l'état miryad-core et le gateway branché sur le SLOT K8s lazy partagé avec
/// `AppState::k8s` — aucun second client, aucun second cache de jeton (`DEC-3`). Construction
/// sans I/O ni échec : la panne de l'hôte est fermée à l'usage, dans ./host.rs.
#[must_use]
pub(crate) fn lymaille_shared_host(state: &AppState) -> Arc<VnlLymailleHost> {
    Arc::new(VnlLymailleHost::new(
        state.auth.db.clone(),
        Arc::new(K8sHostGateway::new(
            Arc::clone(&state.k8s),
            state.config.k8s_namespace.clone(),
        )),
    ))
}

/// Le `SessionManager` de l'app : deps d'assemblage de miryad-core avec l'hôte fourni au
/// montage — `AssembleDeps::host` est le SEUL point où miryad-core 0.4.0 accepte l'hôte
/// (`MiryadAuthState` ne porte aucun champ hôte) — secrets par variable d'environnement
/// (`EnvSecretResolver`) et registre de ressources vide (les modules d'hôte ne sont pas
/// montés dans cette tranche). Séam `pub(crate)` (jurisprudence `start_with`) : les Scenario
/// d'ouverture par écran l'exercent sans servir de HTTP.
pub(crate) fn lymaille_session_manager(
    host: Option<SharedHost>,
    db: DatabaseConnection,
) -> SessionManager {
    let mut assemble = AssembleDeps::new(
        db,
        Arc::new(ResourceRegistry::new()),
        Arc::new(EnvSecretResolver),
        Arc::new(EventHub::new(None, CAPACITE_HUB)),
    );
    assemble.host = host;
    SessionManager::new(ManagerDeps {
        assemble,
        gate: TurnGate::new(),
        turn_lock: None,
        limits: Limits::default(),
    })
}

/// Les routes de conversation de miryad-core (`conversations_router`, préfixe
/// `/api/v1/lymaille` FIGÉ par la crate — l'app ne sert aucune route de conversation propre
/// et ne proxy pas le SSE). Refus fermé (`FR-1`, Scenario « montage refusé sans hôte ») :
/// sans hôte fourni, AUCUNE route n'est montée et l'erreur cite `VNL-APP-SES-109` — la suite
/// des identifiants de montage après `VNL-APP-SES-101..107` (./host.rs) et `108`
/// (./catalog.rs, ./events.rs).
///
/// # Errors
///
/// [`AppError::InternalError`] citant `VNL-APP-SES-109` quand aucun hôte n'est fourni.
pub(crate) fn lymaille_router(
    host: Option<SharedHost>,
    db: DatabaseConnection,
) -> Result<Router<AppState>, AppError> {
    let Some(host) = host else {
        return Err(AppError::InternalError(
            "VNL-APP-SES-109: montage des sessions lymaille refusé — aucun hôte fourni".to_string(),
        ));
    };
    Ok(conversations_router::<AppState>(lymaille_session_manager(
        Some(host),
        db,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::entities::owner_links;
    use crate::db::test_support::real_db;
    use crate::sessions::catalog::{
        G_CONFIG, G_HOME, G_VANYLINE_CONFIG, G_VANYLINE_HOME, G_VANYLINE_PROJECT,
        agent_pour_contexte, ensure_catalog,
    };
    use crate::sessions::host::{HostEntity, HostGatewayError, h_project_id};
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, Set};
    use std::collections::HashSet;
    use std::future::Future;
    use std::pin::Pin;
    use vanyline_lib::k8s::OwnerToken;

    /// Gateway factice de test : seuls les Projects existent (les contextes `H_` d'ouverture
    /// par écran) ; passe par le séam `HostGateway` (convention anti-mock `lib/src/k8s.sdd`).
    struct PorteurFactice {
        projects: HashSet<(String, String)>,
    }

    impl PorteurFactice {
        fn avec_project(owner: &str, nom: &str) -> Self {
            Self {
                projects: HashSet::from([(owner.to_string(), nom.to_string())]),
            }
        }
    }

    impl crate::sessions::host::HostGateway for PorteurFactice {
        fn sandbox_mcp_url<'a>(
            &'a self,
            _owner: &'a str,
            _name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, HostGatewayError>> + Send + 'a>> {
            Box::pin(async { Ok("http://sandbox-web.ana-ns.svc:3000/mcp".to_string()) })
        }
        fn owner_token<'a>(
            &'a self,
            _owner: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<OwnerToken, HostGatewayError>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(OwnerToken {
                    token: "jwt-du-sa-d-ana".to_string(),
                    expires_at: Utc::now(),
                })
            })
        }
        fn sandbox<'a>(
            &'a self,
            _owner: &'a str,
            _name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>
        {
            Box::pin(async { Err(HostGatewayError::Refused) })
        }
        fn project<'a>(
            &'a self,
            owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>
        {
            let existants = &self.projects;
            Box::pin(async move {
                let cle = (owner.to_string(), name.to_string());
                if existants.contains(&cle) {
                    Ok(HostEntity {
                        name: name.to_string(),
                        namespace: Some(format!("vanyline-{owner}")),
                    })
                } else {
                    Err(HostGatewayError::Refused)
                }
            })
        }
    }

    /// Scenario : « montage refusé sans hôte » (`mod.sdd`) — aucun hôte fourni au montage
    /// n'EST PAS une option : `lymaille_router` rend une erreur citant `VNL-APP-SES-109` et
    /// aucune route de conversation n'existe (`FR-1`, échec fermé — le démarrage de l'app
    /// convertit cette erreur en sortie fatale, ./../main.rs).
    #[tokio::test]
    async fn montage_refuse_sans_hote() {
        let db = real_db().await;

        let Err(err) = lymaille_router(None, db) else {
            panic!("un montage sans hôte doit être refusé, jamais servir des routes");
        };
        let rendu = err.to_string();
        assert!(
            rendu.contains("VNL-APP-SES-109"),
            "l'erreur doit porter l'identifiant de montage : {rendu}"
        );
    }

    /// Scenario : « routes sous lymaille et pas ailleurs » (volet montage en base réelle) —
    /// avec l'hôte fourni, le routeur est construit ; la table complète des routes servies
    /// sous `/api/v1/lymaille` et nulle part ailleurs est verrouillée par le test du routeur
    /// complet de ./../main.rs (`routes_sous_lymaille_et_pas_ailleurs`).
    #[tokio::test]
    async fn montage_avec_hote_construit_le_routeur() {
        let db = real_db().await;
        let hote: SharedHost = Arc::new(VnlLymailleHost::new(
            db.clone(),
            Arc::new(PorteurFactice::avec_project("ana", "p")),
        ));

        drop(
            lymaille_router(Some(hote), db).expect("avec l'hôte, le montage construit le routeur"),
        );
    }

    /// Scenario : « ouverture selon l'écran (`AC-4`) » — integration réelle sur Postgres de
    /// test (sqlite mémoire) : `ensure_catalog` puis ouverture par le `SessionManager`
    /// monté, SANS que l'appelant nomme d'agent pour les contextes stockés (`DEC-7`) et avec
    /// l'agent de la table compilée pour les `H_` (`DEC-6`). Le SSE multi-sessions réel et
    /// les tours d'agent relèvent du cluster (aucun fournisseur de modèle monté ici) —
    /// signalé au rapport.
    #[tokio::test]
    async fn ouverture_par_ecran_apres_catalogue() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("catalogue publié");
        let user = miryad_core::users::resolve_user(&db, "sub-ana", None)
            .await
            .expect("utilisateur miryad résolu");
        owner_links::ActiveModel {
            id: NotSet,
            user_id: Set(user.id),
            k8s_owner_name: Set(Some("ana".to_string())),
        }
        .insert(&db)
        .await
        .expect("lien d'owner posé");

        let hote: SharedHost = Arc::new(VnlLymailleHost::new(
            db.clone(),
            Arc::new(PorteurFactice::avec_project("ana", "mon-projet")),
        ));
        let manager = lymaille_session_manager(Some(hote), db.clone());

        // Accueil (`DEC-7`) : nature et agent viennent du type, l'appelant ne nomme rien.
        let accueil = manager
            .open("sub-ana", G_HOME, None)
            .await
            .expect("l'accueil ouvre une session");
        assert_eq!(accueil.kind, lymaille::SessionKind::Work);
        assert_eq!(accueil.agent_id, G_VANYLINE_HOME);
        assert_eq!(accueil.context, G_HOME);

        // Configuration : la nature `Config` et son agent viennent aussi du type.
        let config = manager
            .open("sub-ana", G_CONFIG, None)
            .await
            .expect("la configuration ouvre une session");
        assert_eq!(config.kind, lymaille::SessionKind::Config);
        assert_eq!(config.agent_id, G_VANYLINE_CONFIG);

        // Écran projet (`DEC-6`) : l'identifiant `H_` ouvre avec l'agent de la table
        // compilée — c'est le montage qui le nomme, pas l'utilisateur.
        let id = h_project_id("mon-projet");
        let agent = agent_pour_contexte(&id).expect("l'écran projet désigne son agent");
        let projet = manager
            .open("sub-ana", &id, Some(agent))
            .await
            .expect("le projet ouvre une session sur son H_");
        assert_eq!(projet.kind, lymaille::SessionKind::Work);
        assert_eq!(projet.agent_id, G_VANYLINE_PROJECT);

        // Un `H_` sans agent explicite est refusé par miryad-core : la table compilée est
        // BIEN ce qui rend l'ouverture par écran possible (`DEC-6`).
        assert!(
            manager.open("sub-ana", &id, None).await.is_err(),
            "un contexte hôte sans agent explicite ne doit rien ouvrir"
        );
    }
}
