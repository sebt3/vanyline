//! Client de l'accès fichier de la sandbox (`lib/src/sandbox_fs.sdd`) : un ticket
//! obtenu par `POST /ws/ticket` avec le jeton d'Owner, puis le RPC JSON de `/ws/fs`
//! (une requête = une frame texte = une réponse). Ce module n'émet aucun journal :
//! ni le ticket ni le jeton ne sortent de ce fichier, ni dans `Debug`, ni dans une erreur.

use std::fmt;

use futures::{SinkExt, StreamExt};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Marqueur de troncature de `list` côté sandbox (`sync#CON-2`).
const MARQUEUR_TRONCATURE: &str = "[truncated at";

/// Plafond par défaut de ce que le client accepte de recevoir d'un `read` brut
/// (`sandbox_fs.sdd`, `Handles` ; `sync#CON-4`).
pub const LIMITE_LECTURE_DEFAUT: usize = 1_048_576;

/// Erreurs du client. Aucune ne contient le jeton ni le ticket.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// Ticket refusé : jeton d'Owner refusé par `POST /ws/ticket`, ou ticket expiré ou déjà consommé.
    #[error("VNL-FS-001: ticket refused by the sandbox")]
    Auth,
    /// Échec réseau, statut HTTP inattendu ou schéma non pris en charge. Sans détail : une URL porte le ticket.
    #[error("VNL-FS-002: transport failure with the sandbox")]
    Transport,
    /// Réponse de protocole inattendue, ou erreur de l'opération qui n'entre dans aucune autre variante.
    #[error("VNL-FS-003: protocol error on `{op}`: {message}")]
    Protocol {
        /// Opération en cause (`read`, `delete`, …).
        op: &'static str,
        /// Message de la sandbox, ou description locale du défaut.
        message: String,
    },
    /// Chemin refusé par la sandbox (confinement, `VNL-SBX-001`/`VNL-SBX-003`), ou chemin vide refusé localement.
    #[error("VNL-FS-004: path refused: {0}")]
    Path(String),
    /// Contenu qui n'est pas du texte UTF-8 (`VNL-TLS-009`, ou écriture d'octets non UTF-8).
    #[error("VNL-FS-005: content is not UTF-8 text")]
    NotText,
}

/// Une entrée d'un listage d'un seul niveau.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entree {
    /// Nom de l'entrée, sans `/` final.
    pub nom: String,
    /// Vrai pour un dossier (la sandbox suffixe son nom par `/`).
    pub dossier: bool,
}

/// Résultat de `list` : les entrées, et le marqueur de troncature à 200 entrées (`sync#CON-2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Entrées du dossier, dans l'ordre de la sandbox (trié).
    pub entrees: Vec<Entree>,
    /// Vrai si la sandbox a tronqué le listage à 200 entrées.
    pub tronque: bool,
}

/// Connexion `/ws/fs` ouverte avec un ticket. Une requête à la fois (`&mut self`).
pub struct SandboxFs {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    prochain_id: u64,
    limite_lecture: usize,
}

impl fmt::Debug for SandboxFs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxFs")
            .field("limite_lecture", &self.limite_lecture)
            .finish_non_exhaustive()
    }
}

/// Réponse de `POST /ws/ticket` : `{"ticket":"<opaque>","expires_in_secs":30}`.
#[derive(Deserialize)]
struct ReponseTicket {
    ticket: String,
}

impl SandboxFs {
    /// Obtient un ticket par `POST {base_url}/ws/ticket` avec le jeton d'Owner, puis ouvre
    /// `{base_url}/ws/fs?ticket=…`.
    ///
    /// # Errors
    ///
    /// [`FsError::Auth`] si le jeton ou le ticket est refusé ; [`FsError::Transport`] pour
    /// tout autre échec réseau ou de schéma.
    pub async fn connect(
        base_url: &str,
        jeton_owner: &SecretString,
        http: &reqwest::Client,
    ) -> Result<Self, FsError> {
        let ticket = emettre_ticket(base_url, jeton_owner, http).await?;
        let url = url_ws_fs(base_url, &ticket)?;
        let (ws, _reponse) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(erreur_connexion)?;
        Ok(Self {
            ws,
            prochain_id: 1,
            limite_lecture: LIMITE_LECTURE_DEFAUT,
        })
    }

    /// Définit le plafond de ce que le client accepte de recevoir d'un `read` brut.
    #[must_use]
    pub fn with_read_limit(mut self, octets: usize) -> Self {
        self.limite_lecture = octets;
        self
    }

    /// Lit le contenu intégral d'un fichier (`read` avec `raw: true`).
    ///
    /// # Errors
    ///
    /// [`FsError::Path`] pour un chemin vide ou refusé ; [`FsError::NotText`] si le fichier
    /// n'est pas UTF-8 ; [`FsError::Protocol`] au-delà du plafond de lecture.
    pub async fn read_raw(&mut self, chemin: &str) -> Result<Vec<u8>, FsError> {
        verifier_chemin(chemin)?;
        let reponse = self
            .operation("read", json!({"path": chemin, "raw": true}))
            .await?;
        let contenu = reponse
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| FsError::Protocol {
                op: "read",
                message: "champ content absent".to_owned(),
            })?;
        if contenu.len() > self.limite_lecture {
            return Err(FsError::Protocol {
                op: "read",
                message: format!(
                    "contenu de {} octets au-delà du plafond de {} octets",
                    contenu.len(),
                    self.limite_lecture
                ),
            });
        }
        Ok(contenu.as_bytes().to_vec())
    }

    /// Écrit un fichier (parents créés, écriture atomique côté sandbox).
    ///
    /// # Errors
    ///
    /// [`FsError::NotText`] si `octets` n'est pas UTF-8 ; [`FsError::Path`] ou
    /// [`FsError::Protocol`] si la sandbox refuse l'écriture.
    pub async fn write(&mut self, chemin: &str, octets: &[u8]) -> Result<(), FsError> {
        verifier_chemin(chemin)?;
        let texte = std::str::from_utf8(octets).map_err(|_| FsError::NotText)?;
        self.operation("write", json!({"path": chemin, "content": texte}))
            .await?;
        Ok(())
    }

    /// Supprime un fichier ou un dossier vide. Rend `Ok(false)` si le chemin n'existe pas.
    ///
    /// # Errors
    ///
    /// [`FsError::Path`] ou [`FsError::Protocol`] (dossier non vide, par exemple).
    pub async fn delete(&mut self, chemin: &str) -> Result<bool, FsError> {
        verifier_chemin(chemin)?;
        match self.requete("delete", json!({"path": chemin})).await? {
            Ok(_) => Ok(true),
            Err(message) if message.contains("VNL-TLS-001") => Ok(false),
            Err(message) => Err(erreur_operation("delete", message)),
        }
    }

    /// Crée un dossier, parents compris (idempotent).
    ///
    /// # Errors
    ///
    /// [`FsError::Path`] ou [`FsError::Protocol`] si la sandbox refuse.
    pub async fn mkdir(&mut self, chemin: &str) -> Result<(), FsError> {
        verifier_chemin(chemin)?;
        self.operation("mkdir", json!({"path": chemin})).await?;
        Ok(())
    }

    /// Liste un dossier, un niveau (`list` avec `depth: 1`), bornée à 200 entrées.
    ///
    /// # Errors
    ///
    /// [`FsError::Path`] ou [`FsError::Protocol`] si la sandbox refuse le listage.
    pub async fn list(&mut self, chemin: &str) -> Result<Listing, FsError> {
        verifier_chemin(chemin)?;
        let reponse = self
            .operation("list", json!({"path": chemin, "depth": 1}))
            .await?;
        let texte = reponse
            .get("entries")
            .and_then(Value::as_str)
            .ok_or_else(|| FsError::Protocol {
                op: "list",
                message: "champ entries absent".to_owned(),
            })?;
        Ok(analyser_listage(chemin, texte))
    }

    /// Envoie une requête et rend la réponse `ok:true`, ou l'erreur de la sandbox mappée.
    async fn operation(&mut self, op: &'static str, params: Value) -> Result<Value, FsError> {
        match self.requete(op, params).await? {
            Ok(reponse) => Ok(reponse),
            Err(message) => Err(erreur_operation(op, message)),
        }
    }

    /// Une requête, une réponse (`sandbox/src/ws/fs.sdd`) : les frames de push (`event`) sont
    /// ignorées. Rend `Ok(Err(message))` pour une réponse `ok:false` de la sandbox.
    async fn requete(
        &mut self,
        op: &'static str,
        mut params: Value,
    ) -> Result<Result<Value, String>, FsError> {
        let id = self.prochain_id;
        self.prochain_id = self.prochain_id.saturating_add(1);
        if let Some(obj) = params.as_object_mut() {
            obj.insert("op".to_owned(), json!(op));
            obj.insert("id".to_owned(), json!(id));
        }
        self.ws
            .send(Message::Text(params.to_string().into()))
            .await
            .map_err(|_| FsError::Transport)?;
        loop {
            let Some(Ok(trame)) = self.ws.next().await else {
                return Err(FsError::Transport);
            };
            let texte = match trame {
                Message::Text(texte) => texte,
                Message::Close(_) => return Err(FsError::Transport),
                _ => continue,
            };
            let reponse: Value =
                serde_json::from_str(texte.as_str()).map_err(|_| FsError::Protocol {
                    op,
                    message: "réponse non JSON".to_owned(),
                })?;
            if reponse.get("event").is_some() {
                continue;
            }
            if reponse.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(FsError::Protocol {
                    op,
                    message: "identifiant de réponse inattendu".to_owned(),
                });
            }
            return match reponse.get("ok").and_then(Value::as_bool) {
                Some(true) => Ok(Ok(reponse)),
                Some(false) => Ok(Err(reponse
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("erreur sans message")
                    .to_owned())),
                None => Err(FsError::Protocol {
                    op,
                    message: "réponse sans champ ok".to_owned(),
                }),
            };
        }
    }
}

/// Chemin refusé localement s'il est vide (la racine n'est jamais une cible) ou porte un octet nul.
fn verifier_chemin(chemin: &str) -> Result<(), FsError> {
    if chemin.is_empty() || chemin.contains('\0') {
        return Err(FsError::Path(
            "chemin vide ou contenant un octet nul".to_owned(),
        ));
    }
    Ok(())
}

/// Mappe une erreur `ok:false` de la sandbox sur une variante du client.
fn erreur_operation(op: &'static str, message: String) -> FsError {
    if message.contains("VNL-TLS-009") {
        FsError::NotText
    } else if message.contains("VNL-SBX-001") || message.contains("VNL-SBX-003") {
        FsError::Path(message)
    } else {
        FsError::Protocol { op, message }
    }
}

/// Une erreur de handshake `401` est un ticket refusé ; tout le reste est un échec de transport.
/// Le détail de la bibliothèque n'est pas repris : il peut contenir l'URL, donc le ticket.
fn erreur_connexion(erreur: tungstenite::Error) -> FsError {
    match erreur {
        tungstenite::Error::Http(reponse) if reponse.status().as_u16() == 401 => FsError::Auth,
        _ => FsError::Transport,
    }
}

/// `POST /ws/ticket` avec le jeton d'Owner ; rend le ticket opaque.
async fn emettre_ticket(
    base_url: &str,
    jeton_owner: &SecretString,
    http: &reqwest::Client,
) -> Result<String, FsError> {
    let url = format!("{}/ws/ticket", base_url.trim_end_matches('/'));
    let reponse = http
        .post(url)
        .bearer_auth(jeton_owner.expose_secret())
        .send()
        .await
        .map_err(|_| FsError::Transport)?;
    let statut = reponse.status();
    if statut == reqwest::StatusCode::UNAUTHORIZED || statut == reqwest::StatusCode::FORBIDDEN {
        return Err(FsError::Auth);
    }
    if !statut.is_success() {
        return Err(FsError::Transport);
    }
    let corps: ReponseTicket = reponse.json().await.map_err(|_| FsError::Protocol {
        op: "ws/ticket",
        message: "réponse de POST /ws/ticket illisible".to_owned(),
    })?;
    if corps.ticket.is_empty() || !corps.ticket.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(FsError::Protocol {
            op: "ws/ticket",
            message: "ticket de forme inattendue".to_owned(),
        });
    }
    Ok(corps.ticket)
}

/// Analyse le texte de `list` (un nom par ligne, `/` pour un dossier, marqueur de troncature).
/// Un dossier vide porte le texte `<chemin> is empty`, rendu comme listage vide.
#[must_use]
pub fn analyser_listage(chemin: &str, texte: &str) -> Listing {
    if texte.trim_end() == format!("{chemin} is empty") {
        return Listing {
            entrees: Vec::new(),
            tronque: false,
        };
    }
    let mut tronque = false;
    let mut entrees = Vec::new();
    for ligne in texte.lines() {
        if ligne.starts_with(MARQUEUR_TRONCATURE) {
            tronque = true;
        } else if !ligne.is_empty() && !ligne.starts_with(' ') {
            // Profondeur 1 : aucune ligne indentée n'est attendue.
            match ligne.strip_suffix('/') {
                Some(nom) => entrees.push(Entree {
                    nom: nom.to_owned(),
                    dossier: true,
                }),
                None => entrees.push(Entree {
                    nom: ligne.to_owned(),
                    dossier: false,
                }),
            }
        }
    }
    Listing { entrees, tronque }
}

/// Construit l'URL `ws://…/ws/fs?ticket=…` à partir de `http://…` (ou `ws://…`).
///
/// # Errors
///
/// [`FsError::Transport`] si le schéma n'est ni `http` ni `ws` (TLS non compilé).
pub fn url_ws_fs(base_url: &str, ticket: &str) -> Result<String, FsError> {
    let base = base_url.trim_end_matches('/');
    let reste = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("ws://"))
        .ok_or(FsError::Transport)?;
    Ok(format!("ws://{reste}/ws/fs?ticket={ticket}"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::io;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::ws::{Message, WebSocket};
    use axum::extract::{Query, State, WebSocketUpgrade};
    use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use tracing::instrument::WithSubscriber;

    use super::{Entree, FsError, Listing, SandboxFs, analyser_listage, url_ws_fs};

    const JETON_VALIDE: &str = "jeton-owner-de-test-0123456789";
    const JETON_FAUX: &str = "jeton-faux-de-test-abcdefghij";

    /// État du sandbox simulé : fichiers et dossiers en mémoire, tickets émis et encore valides.
    #[derive(Default)]
    struct Sim {
        fichiers: BTreeMap<String, Vec<u8>>,
        dossiers: BTreeSet<String>,
        tickets_valides: HashSet<String>,
        tickets_emis: Vec<String>,
        pushes_avant_reponse: bool,
    }

    type Partage = Arc<Mutex<Sim>>;

    fn jeton_owner(valeur: &str) -> SecretString {
        SecretString::from(valeur.to_owned())
    }

    /// Lance le sandbox simulé sur un port libre et rend son URL de base.
    async fn lancer_sim(partage: Partage) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("adresse").port();
        let app = Router::new()
            .route("/ws/ticket", post(ticket_handler))
            .route("/ws/fs", get(fs_handler))
            .with_state(partage);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn ticket_handler(State(p): State<Partage>, headers: HeaderMap) -> Response {
        let attendu = format!("Bearer {JETON_VALIDE}");
        let autorise = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == attendu);
        if !autorise {
            return (StatusCode::UNAUTHORIZED, "{\"error\":\"unauthorized\"}").into_response();
        }
        let ticket = uuid::Uuid::new_v4().simple().to_string();
        if let Ok(mut s) = p.lock() {
            s.tickets_valides.insert(ticket.clone());
            s.tickets_emis.push(ticket.clone());
        }
        let corps = format!("{{\"ticket\":\"{ticket}\",\"expires_in_secs\":30}}");
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            corps,
        )
            .into_response()
    }

    async fn fs_handler(
        State(p): State<Partage>,
        Query(q): Query<std::collections::HashMap<String, String>>,
        ws: WebSocketUpgrade,
    ) -> Response {
        let valide = q
            .get("ticket")
            .is_some_and(|t| p.lock().is_ok_and(|mut s| s.tickets_valides.remove(t)));
        if !valide {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ws.on_upgrade(move |socket| sim_session(p, socket))
    }

    async fn sim_session(p: Partage, mut socket: WebSocket) {
        while let Some(Ok(msg)) = socket.recv().await {
            let texte = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let pushes = p.lock().is_ok_and(|s| s.pushes_avant_reponse);
            if pushes
                && socket
                    .send(Message::Text(
                        "{\"event\":\"file-changed\",\"path\":\"x.txt\"}".into(),
                    ))
                    .await
                    .is_err()
            {
                break;
            }
            let reponse = traiter(&p, &texte);
            if socket.send(Message::Text(reponse.into())).await.is_err() {
                break;
            }
        }
    }

    /// Dispatch simulé du protocole de `sandbox/src/ws/fs.sdd` (ops utilisées par le client).
    fn traiter(p: &Partage, texte: &str) -> String {
        let requete: Value = serde_json::from_str(texte).unwrap_or(Value::Null);
        let id = requete.get("id").cloned();
        let Ok(mut s) = p.lock() else {
            return "{\"ok\":false,\"error\":\"poisoned\"}".to_owned();
        };
        let chemin = requete
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let resultat = match requete.get("op").and_then(Value::as_str) {
            Some(op) => executer(&mut s, op, &chemin, &requete),
            None => Err("missing op".to_owned()),
        };
        let mut reponse = match resultat {
            Ok(v) => v,
            Err(e) => json!({"ok": false, "error": e}),
        };
        if let (Some(id), Some(obj)) = (id, reponse.as_object_mut()) {
            obj.insert("id".to_owned(), id);
        }
        reponse.to_string()
    }

    fn normaliser(chemin: &str) -> Result<String, String> {
        if chemin.split('/').any(|c| c == "..") {
            return Err(format!(
                "VNL-SBX-001: path escapes sandbox root: {chemin} (resolved outside /sandbox)"
            ));
        }
        Ok(chemin.trim_matches('/').to_owned())
    }

    // Dispatch de test complet (une branche par op) : une seule fonction, lisible d'un tenant.
    #[allow(clippy::too_many_lines)]
    fn executer(s: &mut Sim, op: &str, chemin: &str, requete: &Value) -> Result<Value, String> {
        let chemin = normaliser(chemin)?;
        match op {
            "read" => {
                if chemin.is_empty() || s.dossiers.contains(&chemin) {
                    return Err(format!("VNL-TLS-002: not a file: {chemin}"));
                }
                let octets = s
                    .fichiers
                    .get(&chemin)
                    .ok_or_else(|| format!("VNL-TLS-001: not found: {chemin}"))?;
                let texte = String::from_utf8(octets.clone()).map_err(|_| {
                    "VNL-TLS-009: I/O error: stream did not contain valid UTF-8".to_owned()
                })?;
                Ok(json!({"ok": true, "content": texte, "truncated": false}))
            }
            "write" => {
                if chemin.is_empty() || s.dossiers.contains(&chemin) {
                    return Err(format!("VNL-TLS-002: not a file: {chemin}"));
                }
                let contenu = requete
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing content".to_owned())?;
                let segments: Vec<&str> = chemin.split('/').collect();
                for i in 1..segments.len() {
                    s.dossiers.insert(segments[..i].join("/"));
                }
                s.fichiers
                    .insert(chemin.clone(), contenu.as_bytes().to_vec());
                Ok(json!({"ok": true, "wrote": format!("/sandbox/{chemin}")}))
            }
            "delete" => {
                if s.fichiers.remove(&chemin).is_some() {
                    return Ok(json!({"ok": true, "deleted": format!("/sandbox/{chemin}")}));
                }
                if s.dossiers.contains(&chemin) {
                    let prefixe = format!("{chemin}/");
                    let non_vide = s.fichiers.keys().any(|k| k.starts_with(&prefixe))
                        || s.dossiers.iter().any(|d| d.starts_with(&prefixe));
                    if non_vide {
                        return Err(format!("VNL-TLS-005: directory is not empty: {chemin}"));
                    }
                    s.dossiers.remove(&chemin);
                    return Ok(json!({"ok": true, "deleted": format!("/sandbox/{chemin}")}));
                }
                Err(format!("VNL-TLS-001: not found: {chemin}"))
            }
            "mkdir" => {
                if s.fichiers.contains_key(&chemin) {
                    return Err(format!("VNL-TLS-003: not a directory: {chemin}"));
                }
                let segments: Vec<&str> = chemin.split('/').collect();
                for i in 1..=segments.len() {
                    s.dossiers.insert(segments[..i].join("/"));
                }
                Ok(json!({"ok": true, "mkdir": format!("/sandbox/{chemin}")}))
            }
            "list" => {
                let prefixe_dossier = format!("{chemin}/");
                let est_dossier = chemin.is_empty()
                    || s.dossiers.contains(&chemin)
                    || s.fichiers.keys().any(|k| k.starts_with(&prefixe_dossier));
                if !est_dossier {
                    if s.fichiers.contains_key(&chemin) {
                        return Err(format!("VNL-TLS-003: not a directory: {chemin}"));
                    }
                    return Err(format!("VNL-TLS-001: not found: {chemin}"));
                }
                let prefixe = if chemin.is_empty() {
                    String::new()
                } else {
                    format!("{chemin}/")
                };
                let mut noms: BTreeSet<String> = BTreeSet::new();
                let chemins = s
                    .fichiers
                    .keys()
                    .cloned()
                    .chain(s.dossiers.iter().cloned())
                    .collect::<Vec<_>>();
                for c in chemins {
                    if let Some(reste) = c.strip_prefix(&prefixe) {
                        if reste.is_empty() {
                            continue;
                        }
                        match reste.split_once('/') {
                            Some((dossier, _)) => noms.insert(format!("{dossier}/")),
                            None if s.dossiers.contains(&c) => noms.insert(format!("{reste}/")),
                            None => noms.insert(reste.to_owned()),
                        };
                    }
                }
                if noms.is_empty() {
                    return Ok(
                        json!({"ok": true, "entries": format!("{chemin} is empty"), "truncated": false}),
                    );
                }
                let total = noms.len();
                let mut lignes: Vec<String> = noms.into_iter().take(200).collect();
                let tronque = total > 200;
                if tronque {
                    lignes.push(format!(
                        "[truncated at 200 entries — {} more]",
                        total.saturating_sub(200)
                    ));
                }
                Ok(json!({"ok": true, "entries": lignes.join("\n"), "truncated": tronque}))
            }
            other => Err(format!("unknown op: {other}")),
        }
    }

    /// Tampon partagé servant de cible au `tracing` de test.
    #[derive(Clone, Default)]
    struct Tampon(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Tampon {
        fn write(&mut self, octets: &[u8]) -> io::Result<usize> {
            if let Ok(mut v) = self.0.lock() {
                v.extend_from_slice(octets);
            }
            Ok(octets.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Tampon {
        type Writer = Tampon;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Scenario `ticket puis lecture brute`.
    #[tokio::test]
    async fn ticket_puis_lecture_brute() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            s.fichiers
                .insert("sub/file.txt".to_owned(), b"hello\nworld\n".to_vec());
        }
        let base = lancer_sim(p.clone()).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let octets = fs.read_raw("sub/file.txt").await.expect("read_raw");
        assert_eq!(octets, b"hello\nworld\n");
    }

    /// Scenario `ticket refusé`.
    #[tokio::test]
    async fn ticket_refuse() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p).await;
        let err = SandboxFs::connect(&base, &jeton_owner(JETON_FAUX), &reqwest::Client::new())
            .await
            .expect_err("doit échouer");
        assert!(matches!(err, FsError::Auth), "obtenu : {err:?}");
        assert!(!err.to_string().contains(JETON_FAUX));
        assert!(!format!("{err:?}").contains(JETON_FAUX));
    }

    /// Scenario `listage tronqué`.
    #[tokio::test]
    async fn listage_tronque() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            for i in 0..250 {
                s.fichiers
                    .insert(format!("grand/f{i:03}.txt"), b"x".to_vec());
            }
        }
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let listing = fs.list("grand").await.expect("list");
        assert!(listing.tronque, "troncature attendue");
        assert_eq!(listing.entrees.len(), 200);
    }

    /// Scenario `ticket jamais journalisé` : aucune trace du ticket ni du jeton dans les journaux.
    #[tokio::test]
    async fn ticket_jamais_journalise() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            s.fichiers.insert("a.txt".to_owned(), b"a".to_vec());
        }
        let base = lancer_sim(p.clone()).await;
        let tampon = Tampon::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(tampon.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let p2 = p.clone();
        async move {
            let mut fs =
                SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
                    .await
                    .expect("connect");
            let _ = fs.read_raw("a.txt").await;
            let _ = fs.write("b.txt", b"b").await;
            let _ = fs.list("").await;
            let _ = fs.delete("b.txt").await;
            // Échec d'authentification côté connect : le journal ne doit rien contenir non plus.
            let _ =
                SandboxFs::connect(&base, &jeton_owner(JETON_FAUX), &reqwest::Client::new()).await;
            let tickets = p2
                .lock()
                .map(|s| s.tickets_emis.clone())
                .unwrap_or_default();
            let journal = tampon
                .0
                .lock()
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .unwrap_or_default();
            assert!(!journal.contains(JETON_VALIDE));
            assert!(!journal.contains(JETON_FAUX));
            for t in tickets {
                assert!(!journal.contains(&t), "ticket dans le journal");
            }
        }
        .with_subscriber(sub)
        .await;
    }

    /// Ticket consommé à l'upgrade : le même ticket ne rouvre pas `/ws/fs`.
    #[tokio::test]
    async fn ticket_consomme_a_l_upgrade() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p.clone()).await;
        let _fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let ticket = p
            .lock()
            .map(|s| s.tickets_emis[0].clone())
            .unwrap_or_default();
        let url = format!("{}/ws/fs?ticket={ticket}", base.replacen("http", "ws", 1));
        let rejeu = tokio_tungstenite::connect_async(url.as_str()).await;
        assert!(
            rejeu.is_err(),
            "un ticket consommé ne doit pas rouvrir /ws/fs"
        );
    }

    /// Scenario `lecture_brute_de_fichier_non_utf8_refusee` (contrat `VNL-TLS-009` -> `NotText`).
    #[tokio::test]
    async fn lecture_non_utf8_refusee() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            s.fichiers
                .insert("bin.dat".to_owned(), vec![0xff, 0xfe, 0x00]);
        }
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let err = fs.read_raw("bin.dat").await.expect_err("doit échouer");
        assert!(matches!(err, FsError::NotText), "obtenu : {err:?}");
    }

    /// Ecriture puis lecture : octets exacts, saut de ligne final compris, parents créés.
    #[tokio::test]
    async fn ecriture_puis_lecture_octets_exacts() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p.clone()).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        fs.write("nouveau/dossier/x.txt", b"a\n\nb\n")
            .await
            .expect("write");
        let octets = fs
            .read_raw("nouveau/dossier/x.txt")
            .await
            .expect("read_raw");
        assert_eq!(octets, b"a\n\nb\n");
    }

    /// Ecriture d'octets non UTF-8 refusée localement (`NotText`), rien n'est envoyé.
    #[tokio::test]
    async fn ecriture_non_utf8_refusee() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p.clone()).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let err = fs
            .write("bin.dat", &[0xff, 0x00])
            .await
            .expect_err("doit échouer");
        assert!(matches!(err, FsError::NotText), "obtenu : {err:?}");
        assert!(p.lock().is_ok_and(|s| s.fichiers.is_empty()));
    }

    /// Chemin vide refusé localement ; chemin hostile refusé par la sandbox (`Path`).
    #[tokio::test]
    async fn chemins_refuses() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        assert!(matches!(fs.read_raw("").await, Err(FsError::Path(_))));
        let hostile = fs
            .read_raw("../../etc/passwd")
            .await
            .expect_err("doit échouer");
        assert!(
            matches!(hostile, FsError::Path(ref m) if m.contains("VNL-SBX-001")),
            "obtenu : {hostile:?}"
        );
    }

    /// Un `read` brut au-delà du plafond de réception est refusé (`Protocol`).
    #[tokio::test]
    async fn lecture_au_dela_du_plafond_refusee() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            s.fichiers
                .insert("long.txt".to_owned(), b"abcdefgh".to_vec());
        }
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect")
            .with_read_limit(4);
        let err = fs.read_raw("long.txt").await.expect_err("doit échouer");
        assert!(
            matches!(err, FsError::Protocol { op: "read", .. }),
            "obtenu : {err:?}"
        );
    }

    /// Suppression : `true` si présent, `false` si absent ; dossier non vide refusé.
    #[tokio::test]
    async fn suppression_fichier_absent_dossier_non_vide() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        fs.write("d/f.txt", b"x").await.expect("write");
        assert!(fs.delete("d/f.txt").await.expect("delete"));
        assert!(!fs.delete("d/f.txt").await.expect("delete absent"));
        fs.write("d/g.txt", b"x").await.expect("write");
        let err = fs.delete("d").await.expect_err("dossier non vide");
        assert!(
            matches!(err, FsError::Protocol { op: "delete", .. }),
            "obtenu : {err:?}"
        );
    }

    /// `mkdir` puis `list` : dossier vide, puis entrées avec marqueur de dossier.
    #[tokio::test]
    async fn mkdir_puis_listage() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        fs.mkdir("a/vide").await.expect("mkdir");
        let vide = fs.list("a/vide").await.expect("list");
        assert_eq!(
            vide,
            Listing {
                entrees: vec![],
                tronque: false
            }
        );
        fs.write("a/f.txt", b"x").await.expect("write");
        let listing = fs.list("a").await.expect("list");
        assert_eq!(
            listing.entrees,
            vec![
                Entree {
                    nom: "f.txt".to_owned(),
                    dossier: false
                },
                Entree {
                    nom: "vide".to_owned(),
                    dossier: true
                },
            ]
        );
        assert!(!listing.tronque);
    }

    /// Les frames de push (`event`) intercalées avant chaque réponse sont ignorées.
    #[tokio::test]
    async fn frames_de_push_ignorees() {
        let p: Partage = Arc::default();
        if let Ok(mut s) = p.lock() {
            s.fichiers.insert("a.txt".to_owned(), b"abc\n".to_vec());
            s.pushes_avant_reponse = true;
        }
        let base = lancer_sim(p).await;
        let mut fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        assert_eq!(fs.read_raw("a.txt").await.expect("read_raw"), b"abc\n");
        assert_eq!(fs.read_raw("a.txt").await.expect("read_raw 2"), b"abc\n");
    }

    /// Le Debug du client ne montre ni jeton ni ticket.
    #[tokio::test]
    async fn debug_sans_secret() {
        let p: Partage = Arc::default();
        let base = lancer_sim(p.clone()).await;
        let fs = SandboxFs::connect(&base, &jeton_owner(JETON_VALIDE), &reqwest::Client::new())
            .await
            .expect("connect");
        let texte = format!("{fs:?}");
        assert!(!texte.contains(JETON_VALIDE));
        let tickets = p.lock().map(|s| s.tickets_emis.clone()).unwrap_or_default();
        for t in tickets {
            assert!(!texte.contains(&t));
        }
    }

    /// URL WebSocket : schéma `http` converti en `ws`, ticket en query ; `https` refusé (TLS non compilé).
    #[test]
    fn url_ws_fs_convertit_le_schema() {
        assert_eq!(
            url_ws_fs("http://sb.ns.svc:3000", "abc").expect("url"),
            "ws://sb.ns.svc:3000/ws/fs?ticket=abc"
        );
        assert_eq!(
            url_ws_fs("http://sb.ns.svc:3000/", "abc").expect("url"),
            "ws://sb.ns.svc:3000/ws/fs?ticket=abc"
        );
        assert!(matches!(
            url_ws_fs("https://sb", "abc"),
            Err(FsError::Transport)
        ));
    }

    /// Analyse du listage : dossiers suffixés, marqueur de troncature retiré, « is empty » vide.
    #[test]
    fn analyse_du_listage() {
        let l = analyser_listage("a", "f.txt\nd/\n[truncated at 200 entries — 3 more]");
        assert!(l.tronque);
        assert_eq!(l.entrees.len(), 2);
        assert!(l.entrees[1].dossier);
        assert_eq!(l.entrees[1].nom, "d");
        let vide = analyser_listage("a", "a is empty");
        assert!(vide.entrees.is_empty() && !vide.tronque);
    }
}
