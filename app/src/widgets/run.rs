//! Route d'exécution d'un widget scripté — contrat : `./run.sdd` (`dsd/app/widgets.md` `FR-2`,
//! `FR-3`, `FR-4`, `FR-6`, `FR-7`, `FR-10`, `DEC-6`). Le script vient de la définition de
//! l'appelant, jamais de la requête ; le contexte vient de l'écran ; le cache et l'annulation
//! appartiennent à `./cache.rs`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value as JsonValue;
use vynil_core::cancel::CancelToken;

use super::cache::{CacheKey, Entry};
use super::model::{RunError, RunRequest, RunResponse};
use super::output::validate_output;
use super::runtime::{WidgetService, panne_interne};
use crate::AppState;
use crate::dashboards::model::{DashboardDef, Scope};
use crate::db::entities::{dashboards, owner_links, projects};
use crate::error::AppError;
use crate::sessions::catalog::G_HOME;
use crate::sessions::{h_project_id, h_sandbox_id};

/// Type de widget qui porte un script (`./run.sdd`, `FR-1`).
const TYPE_SCRIPTE: &str = "scripted";
/// Durée de vie du cache par défaut (`FR-6` : une heure).
const TTL_DEFAUT_SECS: u64 = 3600;

/// `POST /api/widgets/run` (`./run.sdd`) : extrait l'appelant authentifié et l'état, délègue à
/// [`executer`]. Le client ne fournit jamais de script (`DEC-6`).
pub async fn handler_run(
    user: AuthUser,
    State(state): State<AppState>,
    Json(requete): Json<RunRequest>,
) -> Result<Json<RunResponse>, AppError> {
    executer(
        &state.auth.db,
        &state.widgets,
        &user.subject,
        user.email.as_deref(),
        requete,
    )
    .await
    .map(Json)
}

/// Exécution d'un widget scripté pour l'appelant de sujet `subject` (`./run.sdd`). Le script est
/// relu dans la définition de l'appelant ; `requete.sandbox` ne sert qu'à l'écran sandbox.
///
/// # Errors
///
/// `AppError::WidgetNotFound` (404) et `AppError::WidgetContextUnsupported` (422) ; les échecs
/// de script et les pannes internes sont des `RunResponse::Failed` de statut 200.
pub(crate) async fn executer(
    db: &DatabaseConnection,
    service: &WidgetService,
    subject: &str,
    email: Option<&str>,
    requete: RunRequest,
) -> Result<RunResponse, AppError> {
    let appelant = resolve_user(db, subject, email).await?;
    let ligne = dashboards::Entity::find_by_id(requete.dashboard)
        .filter(dashboards::Column::OwnerId.eq(appelant.id))
        .one(db)
        .await?
        .ok_or(AppError::WidgetNotFound)?;
    let Ok(scope) = Scope::from_str(&ligne.scope) else {
        tracing::error!("VNL-APP-WDG-010: écran de tableau illisible en base");
        return Ok(RunResponse::Failed {
            error: panne_interne(),
        });
    };
    let def: DashboardDef = match serde_json::from_value(ligne.def.clone()) {
        Ok(def) => def,
        Err(erreur) => {
            tracing::error!("VNL-APP-WDG-010: définition de tableau illisible: {erreur}");
            return Ok(RunResponse::Failed {
                error: panne_interne(),
            });
        }
    };
    let widget = def
        .widgets
        .iter()
        .find(|w| w.id == requete.widget)
        .filter(|w| w.widget_type == TYPE_SCRIPTE)
        .ok_or(AppError::WidgetNotFound)?;

    let Some(script) = widget
        .config
        .get("script")
        .and_then(JsonValue::as_str)
        .filter(|texte| !texte.is_empty())
        .map(str::to_string)
    else {
        tracing::error!("VNL-APP-WDG-010: widget scripté sans script dans sa définition");
        return Ok(RunResponse::Failed {
            error: panne_interne(),
        });
    };
    let ttl_secs = match widget.config.get("ttl_secs").map(JsonValue::as_u64) {
        None => TTL_DEFAUT_SECS,
        Some(Some(ttl)) => ttl,
        Some(None) => {
            tracing::error!("VNL-APP-WDG-010: ttl_secs du widget n'est pas un entier positif");
            return Ok(RunResponse::Failed {
                error: panne_interne(),
            });
        }
    };

    let contexte = contexte_de(db, appelant.id, scope, &ligne, requete.sandbox.as_deref()).await?;
    let cle = CacheKey {
        subject: subject.to_string(),
        context: contexte.clone(),
        dashboard: ligne.id,
        widget: requete.widget.clone(),
        script_hash: empreinte(&script),
    };
    let calcul_service = service.clone();
    let sujet = subject.to_string();
    let resultat = service
        .cache()
        .get_or_compute(
            cle,
            ttl_secs,
            requete.refresh,
            move |jeton: CancelToken| async move {
                calculer(&calcul_service, &contexte, &sujet, &script, &jeton).await
            },
        )
        .await;

    Ok(match resultat {
        Ok(obtenu) => RunResponse::Ready {
            output: obtenu.entry.output,
            computed_at: obtenu.entry.computed_at.to_rfc3339(),
            duration_ms: obtenu.entry.duration_ms,
            cached: obtenu.cached,
        },
        Err(error) => RunResponse::Failed { error },
    })
}

/// Contexte d'exécution de l'écran (`./run.sdd`, `Handles`) : `G_home` ; `H_project_<nom>` du
/// projet du tableau, relu pour l'appelant ; `H_sandbox_<owner>_<sandbox>`, l'owner K8s lié à
/// l'appelant. Tout ce qui ne se nomme pas est `422` (`WidgetContextUnsupported`).
async fn contexte_de(
    db: &DatabaseConnection,
    owner: i32,
    scope: Scope,
    ligne: &dashboards::Model,
    sandbox: Option<&str>,
) -> Result<String, AppError> {
    match scope {
        Scope::Home => Ok(G_HOME.to_string()),
        Scope::Project => {
            let projet_id = ligne.project_id.ok_or(AppError::WidgetContextUnsupported)?;
            let projet = projects::Entity::find()
                .filter(projects::Column::Id.eq(projet_id))
                .filter(projects::Column::OwnerId.eq(owner))
                .one(db)
                .await?
                .ok_or(AppError::WidgetContextUnsupported)?;
            Ok(h_project_id(&projet.name))
        }
        Scope::Sandbox => {
            let nom = sandbox
                .filter(|nom| !nom.is_empty())
                .ok_or(AppError::WidgetContextUnsupported)?;
            let lien = owner_links::Entity::find()
                .filter(owner_links::Column::UserId.eq(owner))
                .one(db)
                .await?;
            let proprietaire = lien
                .and_then(|lien| lien.k8s_owner_name)
                .ok_or(AppError::WidgetContextUnsupported)?;
            Ok(h_sandbox_id(&proprietaire, nom))
        }
    }
}

/// Un calcul : exécution, validation de la sortie (`007` si refusée), entrée de cache. Le
/// `computed_at` rendu ici est remplacé par l'horloge injectée du cache.
async fn calculer(
    service: &WidgetService,
    contexte: &str,
    sujet: &str,
    script: &str,
    jeton: &CancelToken,
) -> Result<Entry, RunError> {
    let debut = Instant::now();
    let valeur = service.execute(contexte, sujet, script, jeton).await?;
    let output = validate_output(valeur).map_err(|refus| RunError {
        code: "VNL-APP-WDG-007".to_string(),
        message: refus.message.to_string(),
    })?;
    Ok(Entry {
        output,
        computed_at: Utc::now(),
        duration_ms: u64::try_from(debut.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// Empreinte du texte du script : une modification du script change la clé du cache.
fn empreinte(script: &str) -> u64 {
    let mut hacheur = DefaultHasher::new();
    script.hash(&mut hacheur);
    hacheur.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::{dashboards, owner_links, projects};
    use crate::db::test_support::real_db;
    use crate::widgets::cache::{Clock, WidgetCache};
    use crate::widgets::runtime::ScriptExecutor;
    use chrono::Utc;
    use miryad_core::users::resolve_user;
    use sea_orm::{ActiveValue::NotSet, EntityTrait, Set};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    /// Un appel reçu par l'exécuteur factice.
    #[derive(Clone, Debug, PartialEq)]
    struct Appel {
        contexte: String,
        sujet: String,
        source: String,
    }

    /// Exécuteur factice (`./run.sdd`, « exécuteur factice ») : enregistre chaque appel, rend
    /// les réponses de sa file puis un `bar` valide ; `lent` attend l'annulation du jeton.
    struct ExecuteurFactice {
        appels: Mutex<Vec<Appel>>,
        reponses: Mutex<VecDeque<Result<JsonValue, RunError>>>,
        lent: AtomicBool,
    }

    impl ExecuteurFactice {
        fn nouveau() -> Arc<Self> {
            Arc::new(Self {
                appels: Mutex::new(Vec::new()),
                reponses: Mutex::new(VecDeque::new()),
                lent: AtomicBool::new(false),
            })
        }

        fn avec_reponses(reponses: Vec<Result<JsonValue, RunError>>) -> Arc<Self> {
            let exec = Self::nouveau();
            *exec.reponses.lock().unwrap_or_else(PoisonError::into_inner) = reponses.into();
            exec
        }

        fn appels(&self) -> Vec<Appel> {
            self.appels
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    fn sortie_bar() -> JsonValue {
        json!({"type": "bar", "config": {"data": [{"label": "ok", "value": 0.93}]}})
    }

    impl ScriptExecutor for ExecuteurFactice {
        fn execute<'a>(
            &'a self,
            context: &'a str,
            subject: &'a str,
            source: &'a str,
            cancel: &'a CancelToken,
        ) -> Pin<Box<dyn Future<Output = Result<JsonValue, RunError>> + Send + 'a>> {
            Box::pin(async move {
                self.appels
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Appel {
                        contexte: context.to_string(),
                        sujet: subject.to_string(),
                        source: source.to_string(),
                    });
                if self.lent.load(Ordering::SeqCst) {
                    while !cancel.is_cancelled() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    return Err(RunError {
                        code: "VNL-APP-WDG-009".to_string(),
                        message: "exécution annulée".to_string(),
                    });
                }
                let suivante = self
                    .reponses
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .pop_front();
                suivante.unwrap_or_else(|| Ok(sortie_bar()))
            })
        }
    }

    fn service(exec: &Arc<ExecuteurFactice>) -> WidgetService {
        let clock: Clock = Arc::new(Utc::now);
        WidgetService::with_executor(exec.clone(), WidgetCache::new(clock))
    }

    /// Définition d'un tableau : un widget `scripted` (`tests`) et un widget `markdown` (`note`).
    fn def_scriptee(script: &str) -> JsonValue {
        json!({
            "layout": { "columns": 12 },
            "widgets": [
                { "id": "tests", "type": "scripted", "config": { "script": script }, "x": 0, "y": 0, "w": 6, "h": 4 },
                { "id": "note", "type": "markdown", "config": { "text": "perso" }, "x": 6, "y": 0, "w": 6, "h": 4 }
            ]
        })
    }

    async fn semer_projet(db: &DatabaseConnection, owner: i32, nom: &str) -> i32 {
        projects::Entity::insert(projects::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(nom.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("le projet s'insère")
        .last_insert_id
    }

    async fn semer_lien(db: &DatabaseConnection, user_id: i32, owner: &str) {
        owner_links::Entity::insert(owner_links::ActiveModel {
            id: NotSet,
            user_id: Set(user_id),
            k8s_owner_name: Set(Some(owner.to_string())),
        })
        .exec(db)
        .await
        .expect("le lien d'owner s'insère");
    }

    /// Ligne de tableau semée directement (le hook de création est couvert ailleurs).
    async fn semer_tableau(
        db: &DatabaseConnection,
        owner: i32,
        scope: &str,
        project_id: Option<i32>,
        def: JsonValue,
    ) -> i32 {
        dashboards::Entity::insert(dashboards::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            scope: Set(scope.to_string()),
            kind: Set("custom".to_string()),
            project_id: Set(project_id),
            sandbox_id: Set(None),
            title: Set("Tableau".to_string()),
            def: Set(def),
        })
        .exec(db)
        .await
        .expect("le tableau s'insère")
        .last_insert_id
    }

    /// Requête telle que le client l'envoie, avec un champ `script` que la route doit ignorer.
    fn requete(tableau: i32, widget: &str) -> RunRequest {
        serde_json::from_value(json!({"dashboard": tableau, "widget": widget}))
            .expect("requête minimale")
    }

    /// Scenario: un widget scripté d'un tableau d'accueil rend une sortie prête, puis le cache.
    #[tokio::test]
    async fn un_widget_d_accueil_rend_une_sortie_prete_puis_le_cache() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let tableau = semer_tableau(&db, ana.id, "home", None, def_scriptee("40 + 2")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        let premiere = executer(&db, &service, "ana", None, requete(tableau, "tests"))
            .await
            .expect("route");
        let seconde = executer(&db, &service, "ana", None, requete(tableau, "tests"))
            .await
            .expect("route");

        let (
            RunResponse::Ready {
                output: sortie_1,
                cached: cache_1,
                ..
            },
            RunResponse::Ready {
                output: sortie_2,
                cached: cache_2,
                ..
            },
        ) = (premiere, seconde)
        else {
            panic!("deux réponses prêtes attendues");
        };
        assert_eq!(sortie_1.widget_type, "bar");
        assert_eq!(sortie_1.config, sortie_2.config);
        assert!(!cache_1, "premier appel calculé");
        assert!(cache_2, "second appel servi depuis le cache");
        assert_eq!(
            exec.appels().len(),
            1,
            "l'exécuteur n'est appelé qu'une fois"
        );
    }

    /// Scenario: actualiser relance.
    #[tokio::test]
    async fn actualiser_relance() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let tableau = semer_tableau(&db, ana.id, "home", None, def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        executer(&db, &service, "ana", None, requete(tableau, "tests"))
            .await
            .expect("route");
        let rafraichie: RunRequest = serde_json::from_value(json!({
            "dashboard": tableau, "widget": "tests", "refresh": true
        }))
        .expect("requête avec refresh");
        executer(&db, &service, "ana", None, rafraichie)
            .await
            .expect("route");

        assert_eq!(exec.appels().len(), 2, "refresh relance l'exécution");
    }

    /// Scenario: le script de la requête est ignoré.
    #[tokio::test]
    async fn le_script_de_la_requete_est_ignore() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let tableau =
            semer_tableau(&db, ana.id, "home", None, def_scriptee("tableau-script")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);
        let avec_script: RunRequest = serde_json::from_value(json!({
            "dashboard": tableau, "widget": "tests", "script": "loop {}"
        }))
        .expect("requête");

        executer(&db, &service, "ana", None, avec_script)
            .await
            .expect("route");

        let appels = exec.appels();
        assert_eq!(appels.len(), 1);
        assert_eq!(
            appels[0].source, "tableau-script",
            "script de la définition"
        );
    }

    /// Scenario: le tableau ou le widget d'autrui est introuvable.
    #[tokio::test]
    async fn le_tableau_ou_le_widget_d_autrui_est_introuvable() {
        let db = real_db().await;
        let ben = resolve_user(&db, "ben", None).await.expect("compte de ben");
        resolve_user(&db, "ana", None).await.expect("compte d'ana");
        let tableau_de_ben = semer_tableau(&db, ben.id, "home", None, def_scriptee("1")).await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        // Un widget `scripted` (`tests`) et un widget `markdown` (`note`) dans le même tableau.
        let tableau_d_ana = semer_tableau(&db, ana.id, "home", None, def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        let autrui = executer(&db, &service, "ana", None, requete(tableau_de_ben, "tests")).await;
        let absent = executer(&db, &service, "ana", None, requete(tableau_d_ana, "absent")).await;
        let non_scripte =
            executer(&db, &service, "ana", None, requete(tableau_d_ana, "note")).await;

        for resultat in [autrui, absent, non_scripte] {
            assert!(
                matches!(resultat, Err(AppError::WidgetNotFound)),
                "404 VNL-APP-WDG-001 attendu"
            );
        }
        assert!(exec.appels().is_empty(), "rien n'est exécuté");
    }

    /// Scenario: les écrans projet et sandbox s'exécutent dans leur contexte `H_…`.
    #[tokio::test]
    async fn les_ecrans_projet_et_sandbox_s_executent_dans_leur_contexte() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        semer_lien(&db, ana.id, "ana-owner").await;
        let projet = semer_projet(&db, ana.id, "web").await;
        let tableau_projet =
            semer_tableau(&db, ana.id, "project", Some(projet), def_scriptee("1")).await;
        let tableau_sandbox =
            semer_tableau(&db, ana.id, "sandbox", Some(projet), def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        executer(&db, &service, "ana", None, requete(tableau_projet, "tests"))
            .await
            .expect("route projet");
        let mut sandbox: RunRequest = requete(tableau_sandbox, "tests");
        sandbox.sandbox = Some("dev".to_string());
        executer(&db, &service, "ana", None, sandbox)
            .await
            .expect("route sandbox");

        let contextes: Vec<String> = exec.appels().into_iter().map(|a| a.contexte).collect();
        assert_eq!(
            contextes,
            vec![
                "H_project_web".to_string(),
                "H_sandbox_ana-owner_dev".to_string()
            ]
        );
    }

    /// Scenario: un contexte d'écran impossible à nommer est refusé.
    #[tokio::test]
    async fn un_contexte_d_ecran_impossible_a_nommer_est_refuse() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let ben = resolve_user(&db, "ben", None).await.expect("compte de ben");
        // Projet appartenant à un autre compte : introuvable pour ana.
        let projet_de_ben = semer_projet(&db, ben.id, "prive").await;
        let projet_a = semer_projet(&db, ana.id, "a").await;
        let projet_b = semer_projet(&db, ana.id, "b").await;
        let tableau_sans_sandbox =
            semer_tableau(&db, ana.id, "sandbox", Some(projet_a), def_scriptee("1")).await;
        let tableau_projet_disparu = semer_tableau(
            &db,
            ana.id,
            "project",
            Some(projet_de_ben),
            def_scriptee("1"),
        )
        .await;
        let tableau_sandbox_sans_lien =
            semer_tableau(&db, ana.id, "sandbox", Some(projet_b), def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        let mut sans_sandbox = requete(tableau_sans_sandbox, "tests");
        sans_sandbox.sandbox = None;
        let mut sandbox_sans_lien = requete(tableau_sandbox_sans_lien, "tests");
        sandbox_sans_lien.sandbox = Some("dev".to_string());

        let a = executer(&db, &service, "ana", None, sans_sandbox).await;
        let b = executer(
            &db,
            &service,
            "ana",
            None,
            requete(tableau_projet_disparu, "tests"),
        )
        .await;
        let c = executer(&db, &service, "ana", None, sandbox_sans_lien).await;

        for resultat in [a, b, c] {
            assert!(
                matches!(resultat, Err(AppError::WidgetContextUnsupported)),
                "422 VNL-APP-WDG-002 attendu"
            );
        }
    }

    /// Scenario: l'abandon du seul client annule l'exécution : le cache annule le jeton quand le
    /// dernier demandeur part (`cache.sdd`), l'exécuteur s'arrête, et rien n'est mis en cache.
    #[tokio::test]
    async fn l_abandon_du_seul_client_annule_l_execution() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let tableau = semer_tableau(&db, ana.id, "home", None, def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        exec.lent.store(true, Ordering::SeqCst);
        let service = service(&exec);

        let demandeur = {
            let db = db.clone();
            let service = service.clone();
            tokio::spawn(async move {
                executer(&db, &service, "ana", None, requete(tableau, "tests")).await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        demandeur.abort();
        let _ = demandeur.await;
        // Laisse l'exécution observer l'annulation et s'achever.
        tokio::time::sleep(Duration::from_millis(300)).await;

        exec.lent.store(false, Ordering::SeqCst);
        let suivant = executer(&db, &service, "ana", None, requete(tableau, "tests"))
            .await
            .expect("route");
        assert!(
            matches!(suivant, RunResponse::Ready { cached: false, .. }),
            "rien n'a été mis en cache : l'appel suivant recalcule"
        );
        assert_eq!(exec.appels().len(), 2);
    }

    /// Scenario: deux contextes d'un même tableau ne partagent pas le cache.
    #[tokio::test]
    async fn deux_contextes_d_un_meme_tableau_ne_partagent_pas_le_cache() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        semer_lien(&db, ana.id, "ana-owner").await;
        let projet = semer_projet(&db, ana.id, "web").await;
        let tableau = semer_tableau(&db, ana.id, "sandbox", Some(projet), def_scriptee("1")).await;
        let exec = ExecuteurFactice::nouveau();
        let service = service(&exec);

        for sandbox in ["dev", "web"] {
            let mut req = requete(tableau, "tests");
            req.sandbox = Some(sandbox.to_string());
            executer(&db, &service, "ana", None, req)
                .await
                .expect("route");
        }

        let contextes: Vec<String> = exec.appels().into_iter().map(|a| a.contexte).collect();
        assert_eq!(
            contextes,
            vec![
                "H_sandbox_ana-owner_dev".to_string(),
                "H_sandbox_ana-owner_web".to_string()
            ]
        );
    }

    /// Scenario: un script en échec et une sortie hors contrat sont rendus en échec, sans cache.
    #[tokio::test]
    async fn un_echec_et_une_sortie_hors_contrat_sont_rendus_en_echec_sans_cache() {
        let db = real_db().await;
        let ana = resolve_user(&db, "ana", None).await.expect("compte");
        let tableau = semer_tableau(&db, ana.id, "home", None, def_scriptee("1")).await;
        let echec_003 = Err(RunError {
            code: "VNL-APP-WDG-003".to_string(),
            message: "script en échec".to_string(),
        });
        let markdown = Ok(json!({"type": "markdown", "config": {}}));
        let exec = ExecuteurFactice::avec_reponses(vec![
            echec_003.clone(),
            markdown.clone(),
            echec_003,
            markdown,
        ]);
        let service = service(&exec);

        let mut codes = Vec::new();
        for _ in 0..4 {
            let reponse = executer(&db, &service, "ana", None, requete(tableau, "tests"))
                .await
                .expect("route");
            let RunResponse::Failed { error } = reponse else {
                panic!("un échec attendu");
            };
            codes.push(error.code);
        }

        assert_eq!(
            codes,
            vec![
                "VNL-APP-WDG-003".to_string(),
                "VNL-APP-WDG-007".to_string(),
                "VNL-APP-WDG-003".to_string(),
                "VNL-APP-WDG-007".to_string(),
            ]
        );
        assert_eq!(
            exec.appels().len(),
            4,
            "l'exécuteur est rappelé à chaque fois"
        );
    }
}
