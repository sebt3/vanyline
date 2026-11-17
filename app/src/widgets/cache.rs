//! Cache des résultats de widgets scriptés — contrat : `./cache.sdd`
//! (`dsd/app/widgets.md` `FR-6`, `DEC-3`, `CON-4`). Mémoire du processus, par sujet, tableau
//! et widget ; l'horloge est injectée (seule source de `computed_at` et de l'âge).

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use futures::FutureExt;
use tokio::sync::Notify;

use super::model::WidgetOutput;

/// Le calcul s'est arrêté sans résultat (panique de `compute`) : l'appelant traduit ce
/// marqueur dans son erreur (`cache.sdd`, `010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputeAborted;

/// Horloge injectée : seule source de `computed_at` et de l'âge d'une entrée.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Clé du cache : sujet, tableau, widget et empreinte du script (`FR-6`, `CON-3`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Sujet authentifié : une entrée n'est jamais partagée entre sujets.
    pub subject: String,
    /// Identifiant de ligne du tableau.
    pub dashboard: i32,
    /// Identifiant du widget dans le tableau.
    pub widget: String,
    /// Empreinte du script : un script modifié change de clé.
    pub script_hash: u64,
}

/// Résultat calculé d'un widget : sortie, instant du calcul et durée (`FR-6`).
#[derive(Clone, Debug)]
pub struct Entry {
    /// Sortie validée du script.
    pub output: WidgetOutput,
    /// Instant du calcul, fourni par l'horloge injectée.
    pub computed_at: DateTime<Utc>,
    /// Durée du dernier calcul, en millisecondes.
    pub duration_ms: u64,
}

/// Résultat d'un appel : l'entrée et si elle vient du cache (`cached` vrai seulement dans ce cas).
#[derive(Clone, Debug)]
pub struct Fetched {
    /// Entrée rendue.
    pub entry: Entry,
    /// Vrai quand l'entrée est servie depuis le cache.
    pub cached: bool,
}

/// Issue d'une exécution partagée. `Mutex<T>` n'exige que `T: Send` : pas de `E: Sync`.
enum Outcome<E> {
    Pending,
    Done(Result<Entry, E>),
}

/// Emplacement partagé d'une exécution : son issue et le réveil des demandeurs.
struct Slot<E> {
    outcome: Mutex<Outcome<E>>,
    done: Notify,
}

/// Exécution en cours pour une clé, partagée entre demandeurs concurrents.
struct Flight<E> {
    slot: Arc<Slot<E>>,
    /// Nombre de demandeurs encore en attente (décrémenté à l'abandon par `Waiter`).
    waiters: Arc<AtomicUsize>,
}

/// Compte un demandeur vivant : son abandon (drop) le retire de l'exécution partagée.
struct Waiter(Arc<AtomicUsize>);

impl Drop for Waiter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<CacheKey, Entry>,
    /// Exécutions en cours, par clé ; type effacé pour porter le `E` de l'appel.
    flights: HashMap<CacheKey, Box<dyn Any + Send>>,
}

/// Cache des résultats de widgets : handle `Clone` à état partagé (`FR-6`).
#[derive(Clone)]
pub struct WidgetCache {
    inner: Arc<Mutex<State>>,
    clock: Clock,
}

impl WidgetCache {
    /// Crée un cache vide lisant le temps via `clock`.
    #[must_use]
    pub fn new(clock: Clock) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
            clock,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Rend l'entrée servie depuis le cache si elle est dans sa durée de vie et que
    /// `refresh` est faux ; sinon exécute `compute` (partagé avec les demandes concurrentes
    /// sur la même clé). Seules les réussites sont conservées (`cache.sdd`).
    pub async fn get_or_compute<E, F, Fut>(
        &self,
        key: CacheKey,
        ttl_secs: u64,
        refresh: bool,
        compute: F,
    ) -> Result<Fetched, E>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Entry, E>> + Send + 'static,
        E: Clone + Send + 'static + From<ComputeAborted>,
    {
        let (slot, _waiter) = {
            let mut state = self.lock();
            let now = (self.clock)();
            let fresh = if ttl_secs > 0 && !refresh {
                state
                    .entries
                    .get(&key)
                    .filter(|entry| is_fresh(now, entry.computed_at, ttl_secs))
            } else {
                None
            };
            if let Some(entry) = fresh {
                return Ok(Fetched {
                    entry: entry.clone(),
                    cached: true,
                });
            }
            state.entries.remove(&key);
            if let Some(flight) = state
                .flights
                .get(&key)
                .and_then(|boxed| boxed.downcast_ref::<Flight<E>>())
            {
                flight.waiters.fetch_add(1, Ordering::SeqCst);
                (Arc::clone(&flight.slot), Waiter(flight.waiters.clone()))
            } else {
                let slot = Arc::new(Slot {
                    outcome: Mutex::new(Outcome::Pending),
                    done: Notify::new(),
                });
                let waiters = Arc::new(AtomicUsize::new(1));
                let waiter = Waiter(waiters.clone());
                if !state.flights.contains_key(&key) {
                    state.flights.insert(
                        key.clone(),
                        Box::new(Flight {
                            slot: Arc::clone(&slot),
                            waiters,
                        }),
                    );
                }
                let cache = self.clone();
                let task_key = key.clone();
                let task_slot = Arc::clone(&slot);
                tokio::spawn(async move {
                    match AssertUnwindSafe(compute()).catch_unwind().await {
                        Ok(result) => cache.finish(&task_key, ttl_secs, &task_slot, result),
                        Err(_) => cache.abandon::<E>(&task_key, &task_slot),
                    }
                });
                (slot, waiter)
            }
        };
        loop {
            // Réveil créé avant la lecture : une publication entre les deux n'est pas perdue.
            let notified = slot.done.notified();
            {
                let outcome = slot.outcome.lock().unwrap_or_else(PoisonError::into_inner);
                match &*outcome {
                    Outcome::Pending => {}
                    Outcome::Done(result) => {
                        return result.clone().map(|entry| Fetched {
                            entry,
                            cached: false,
                        });
                    }
                }
            }
            notified.await;
        }
    }

    /// Fin d'une exécution réussie ou échouée : retire l'exécution en cours, conserve une
    /// réussite seulement si un demandeur attend encore, puis la publie aux demandeurs.
    fn finish<E>(&self, key: &CacheKey, ttl_secs: u64, slot: &Slot<E>, mut result: Result<Entry, E>)
    where
        E: Clone + Send + 'static,
    {
        if let Ok(entry) = result.as_mut() {
            entry.computed_at = (self.clock)();
        }
        {
            let mut state = self.lock();
            let registered = state
                .flights
                .get(key)
                .is_some_and(|boxed| boxed.is::<Flight<E>>());
            let has_waiters = if registered {
                state
                    .flights
                    .remove(key)
                    .and_then(|boxed| boxed.downcast::<Flight<E>>().ok())
                    .is_some_and(|flight| flight.waiters.load(Ordering::SeqCst) > 0)
            } else {
                true
            };
            if let (Ok(entry), true) = (&result, ttl_secs > 0 && has_waiters) {
                state.entries.insert(key.clone(), entry.clone());
            }
        }
        *slot.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Outcome::Done(result);
        slot.done.notify_waiters();
    }

    /// Panne du calcul : retire l'exécution en cours, sans entrée, et publie
    /// `E::from(ComputeAborted)` aux demandeurs (`cache.sdd`).
    fn abandon<E>(&self, key: &CacheKey, slot: &Slot<E>)
    where
        E: From<ComputeAborted> + 'static,
    {
        {
            let mut state = self.lock();
            if state
                .flights
                .get(key)
                .is_some_and(|boxed| boxed.is::<Flight<E>>())
            {
                state.flights.remove(key);
            }
        }
        *slot.outcome.lock().unwrap_or_else(PoisonError::into_inner) =
            Outcome::Done(Err(E::from(ComputeAborted)));
        slot.done.notify_waiters();
    }
}

/// Une entrée est fraîche si son âge est strictement inférieur à `ttl_secs`.
fn is_fresh(now: DateTime<Utc>, computed_at: DateTime<Utc>, ttl_secs: u64) -> bool {
    let age = now.signed_duration_since(computed_at);
    match chrono::Duration::from_std(std::time::Duration::from_secs(ttl_secs)) {
        Ok(ttl) => age < ttl,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration as StdDuration;

    /// Horloge réglable : les tests avancent le temps sans attendre.
    struct TestClock(Arc<Mutex<DateTime<Utc>>>);

    impl TestClock {
        fn new() -> Self {
            let t0 = DateTime::parse_from_rfc3339("2026-11-17T10:00:00Z")
                .expect("date fixe")
                .with_timezone(&Utc);
            Self(Arc::new(Mutex::new(t0)))
        }

        fn advance(&self, secs: i64) {
            let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            *now = now
                .checked_add_signed(chrono::Duration::seconds(secs))
                .unwrap_or(*now);
        }

        fn clock(&self) -> Clock {
            let now = self.0.clone();
            Arc::new(move || *now.lock().unwrap_or_else(PoisonError::into_inner))
        }
    }

    fn key(subject: &str, dashboard: i32, widget: &str, script_hash: u64) -> CacheKey {
        CacheKey {
            subject: subject.to_string(),
            dashboard,
            widget: widget.to_string(),
            script_hash,
        }
    }

    fn entry() -> Entry {
        Entry {
            output: WidgetOutput {
                widget_type: "bar".to_string(),
                config: json!({"data": [{"label": "ok", "value": 1.0}]}),
            },
            computed_at: DateTime::<Utc>::UNIX_EPOCH,
            duration_ms: 120,
        }
    }

    /// Erreur de test : porte la traduction de `ComputeAborted` exigée par le cache.
    #[derive(Debug, Clone, PartialEq)]
    enum TestErr {
        Aborted,
        Other(String),
    }

    impl From<ComputeAborted> for TestErr {
        fn from(_: ComputeAborted) -> Self {
            Self::Aborted
        }
    }

    /// Compte les appels de `compute` et rend une entrée valide après `delay`.
    fn counted(
        calls: &Arc<AtomicUsize>,
        delay: StdDuration,
    ) -> impl FnOnce() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Entry, TestErr>> + Send>,
    > + Send
    + 'static {
        let calls = calls.clone();
        move || {
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                Ok(entry())
            })
        }
    }

    /// Scenario: le second appel dans la durée de vie ne recalcule pas.
    #[tokio::test(start_paused = true)]
    async fn le_second_appel_dans_la_duree_de_vie_ne_recalcule_pas() {
        let horloge = TestClock::new();
        let cache = WidgetCache::new(horloge.clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);

        cache
            .get_or_compute(k.clone(), 3600, false, counted(&calls, StdDuration::ZERO))
            .await
            .expect("premier calcul");
        horloge.advance(10);
        let second = cache
            .get_or_compute(k, 3600, false, counted(&calls, StdDuration::ZERO))
            .await
            .expect("second appel");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "compute ne doit pas être rappelé"
        );
        assert!(second.cached, "l'entrée rendue vient du cache");
    }

    /// Scenario: un autre utilisateur ne reçoit pas le résultat du premier.
    #[tokio::test(start_paused = true)]
    async fn un_autre_utilisateur_ne_recoit_pas_le_resultat_du_premier() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));

        cache
            .get_or_compute(
                key("ana", 7, "w", 1),
                3600,
                false,
                counted(&calls, StdDuration::ZERO),
            )
            .await
            .expect("calcul de ana");
        let ben = cache
            .get_or_compute(
                key("ben", 7, "w", 1),
                3600,
                false,
                counted(&calls, StdDuration::ZERO),
            )
            .await
            .expect("calcul de ben");

        assert_eq!(calls.load(Ordering::SeqCst), 2, "compute appelé pour ben");
        assert!(!ben.cached);
    }

    /// Scenario: modifier le script ou actualiser relance.
    #[tokio::test(start_paused = true)]
    async fn modifier_le_script_ou_actualiser_relance() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));

        cache
            .get_or_compute(
                key("ana", 7, "w", 1),
                3600,
                false,
                counted(&calls, StdDuration::ZERO),
            )
            .await
            .expect("premier calcul");
        // Script modifié : nouvelle clé, pas de rafraîchissement demandé.
        cache
            .get_or_compute(
                key("ana", 7, "w", 2),
                3600,
                false,
                counted(&calls, StdDuration::ZERO),
            )
            .await
            .expect("script modifié");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "changement de script_hash relance"
        );

        // Même clé, actualisation demandée.
        cache
            .get_or_compute(
                key("ana", 7, "w", 2),
                3600,
                true,
                counted(&calls, StdDuration::ZERO),
            )
            .await
            .expect("actualisation");
        assert_eq!(calls.load(Ordering::SeqCst), 3, "refresh relance");
    }

    /// Scenario: des demandes concurrentes partagent une exécution.
    #[tokio::test(start_paused = true)]
    async fn des_demandes_concurrentes_partagent_une_execution() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);

        let mut taches = Vec::new();
        for _ in 0..10 {
            let cache = cache.clone();
            let k = k.clone();
            let compute = counted(&calls, StdDuration::from_secs(1));
            taches.push(tokio::spawn(async move {
                cache.get_or_compute(k, 3600, false, compute).await
            }));
        }
        let mut resultats = Vec::new();
        for tache in taches {
            resultats.push(tache.await.expect("tâche jointe").expect("calcul réussi"));
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "compute appelé une seule fois"
        );
        assert_eq!(resultats.len(), 10);
        assert!(resultats.iter().all(|f| !f.cached));
        assert!(
            resultats
                .iter()
                .all(|f| f.entry.computed_at == resultats[0].entry.computed_at)
        );
    }

    /// Scenario: l'abandon de tous les demandeurs ne laisse pas d'entrée.
    #[tokio::test(start_paused = true)]
    async fn l_abandon_de_tous_les_demandeurs_ne_laisse_pas_d_entree() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);

        let demandeur = {
            let cache = cache.clone();
            let k = k.clone();
            let compute = counted(&calls, StdDuration::from_secs(5));
            tokio::spawn(async move { cache.get_or_compute(k, 3600, false, compute).await })
        };
        tokio::time::sleep(StdDuration::from_secs(1)).await;
        demandeur.abort();
        // L'exécution s'achève après l'abandon, sans demandeur.
        tokio::time::sleep(StdDuration::from_secs(10)).await;

        cache
            .get_or_compute(k, 3600, false, counted(&calls, StdDuration::ZERO))
            .await
            .expect("appel suivant");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "l'appel suivant recalcule");
    }

    /// Scenario: un échec n'est pas conservé.
    #[tokio::test(start_paused = true)]
    async fn un_echec_n_est_pas_conserve() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);

        let echec = {
            let calls = calls.clone();
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<Entry, TestErr>(TestErr::Other("script en échec".to_string()))
            }
        };
        assert!(
            cache
                .get_or_compute(k.clone(), 3600, false, echec.clone())
                .await
                .is_err()
        );
        assert!(cache.get_or_compute(k, 3600, false, echec).await.is_err());

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "l'échec n'est pas servi depuis le cache"
        );
    }

    /// Scenario: un calcul qui panique rend une erreur, sans entrée (`cache.sdd`,
    /// `ComputeAborted`) : les deux appels rendent `E::from(ComputeAborted)` et `compute`
    /// est appelé les deux fois.
    #[tokio::test(start_paused = true)]
    async fn un_calcul_qui_panique_rend_une_erreur_sans_entree() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);
        let panique = {
            let calls = calls.clone();
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("calcul en panne");
                #[allow(unreachable_code)]
                Ok::<Entry, TestErr>(entry())
            }
        };

        let premier = cache
            .get_or_compute(k.clone(), 3600, false, panique.clone())
            .await;
        let second = cache.get_or_compute(k, 3600, false, panique).await;

        assert_eq!(premier.err(), Some(TestErr::Aborted));
        assert_eq!(second.err(), Some(TestErr::Aborted));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "compute appelé les deux fois"
        );
    }

    /// `Must` : `ttl_secs` `0` désactive le cache (toujours recalculer).
    #[tokio::test(start_paused = true)]
    async fn ttl_zero_desactive_le_cache() {
        let cache = WidgetCache::new(TestClock::new().clock());
        let calls = Arc::new(AtomicUsize::new(0));
        let k = key("ana", 7, "w", 1);

        cache
            .get_or_compute(k.clone(), 0, false, counted(&calls, StdDuration::ZERO))
            .await
            .expect("premier");
        cache
            .get_or_compute(k, 0, false, counted(&calls, StdDuration::ZERO))
            .await
            .expect("second");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
