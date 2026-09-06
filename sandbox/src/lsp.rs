//! Gestion des process LSP par toolchain : spawn, framing `Content-Length`,
//! multiplexage multi-clients. Un process PRIMAIRE par toolchain, partagé entre
//! l'éditeur (route WS /ws/lsp/:toolchain) et les tools MCP `lsp_*`, plus des
//! enfants auxiliaires optionnels (`aux`, multiplexeur — cf.
//! `docs/features/vue-lsp.md`). `aux` vide ⟹ chemin mono-process strictement
//! inchangé (cas de toutes les toolchains sans composite aujourd'hui).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Notify, mpsc};

/// Rôles `aux` réellement spawnés. `docker-lsp` y ajoutera `diagnostics-merge` ;
/// un rôle inconnu au spawn = `tracing::warn!` + enfant ignoré (jamais une erreur
/// de session — invariant 1 de la tâche 03).
const KNOWN_AUX_ROLES: &[&str] = &["tsserver-forward"];

fn is_known_aux_role(role: &str) -> bool {
    KNOWN_AUX_ROLES.contains(&role)
}

/// Spécification d'un processus LSP auxiliaire d'une toolchain composite.
/// Rôles connus : `"tsserver-forward"` (composite Volar — les
/// `tsserver/request` du primaire y sont exécutés via `executeCommand
/// typescript.tsserverRequest` et dépilés en `tsserver/response`, tâche 05).
/// `aux` n'est jamais déclaré par l'utilisateur (preset-only, décision
/// 2026-09-06) — seule la tâche controller en émet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspAux {
    pub role: String,
    pub bin: String,
    pub args: Vec<String>,
    /// Fusionné dans `params.initializationOptions` de l'`initialize` envoyé
    /// à CET enfant (ses clés gagnent en cas de collision avec celles du
    /// client). Clé JSON : `initOptions`.
    #[serde(rename = "initOptions", default)]
    pub init_options: Value,
}

/// Spec d'une toolchain LSP : `bin` est un chemin absolu dans le volume toolchain
/// monté. Lue depuis `VNL_LSP_TOOLCHAINS` (JSON array).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspToolchain {
    pub name: String,
    pub bin: String,
    pub args: Vec<String>,
    /// Enfant(s) auxiliaire(s) du multiplexeur. Absent/vide => session
    /// mono-process au comportement strictement actuel.
    #[serde(default)]
    pub aux: Vec<LspAux>,
}

/// Parse le JSON de `VNL_LSP_TOOLCHAINS` en specs. Erreur si le JSON est invalide
/// ou malformé (`anyhow::Error` avec contexte).
pub fn parse_lsp_toolchains(json: &str) -> anyhow::Result<Vec<LspToolchain>> {
    let specs: Vec<LspToolchain> = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("LSP toolchain parse error: {e}"))?;
    Ok(specs)
}

/// Lit `VNL_LSP_TOOLCHAINS` (env). Env absente → `Ok(vec![])` (aucune toolchain LSP).
pub fn lsp_toolchains_from_env() -> anyhow::Result<Vec<LspToolchain>> {
    let json = std::env::var("VNL_LSP_TOOLCHAINS").unwrap_or_default();
    if json.is_empty() {
        return Ok(vec![]);
    }
    parse_lsp_toolchains(&json)
}

/// Encode un payload JSON-RPC en trame stdio LSP : `Content-Length: N\r\n\r\n` + payload.
pub fn encode_message(payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let header = format!("Content-Length: {len}\r\n\r\n");
    let mut buf = Vec::with_capacity(header.len() + len);
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Décodeur incrémental de trames `Content-Length` sur un flux d'octets.
pub struct FrameReader {
    buf: Vec<u8>,
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReader {
    pub fn new() -> Self {
        FrameReader { buf: Vec::new() }
    }

    /// Ajoute un chunk d'octets au buffer interne.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Rend la première trame complète (`payload` pur), ou `None`.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        // Chercher le terminateur \r\n\r\n
        let pos = self.buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let header_bytes = &self.buf[..pos];
        let body_start = pos + 4;

        // Décoder le header (insensible à la casse)
        let header_str = String::from_utf8_lossy(header_bytes);

        // Chercher content-length: dans le header (insensible à la casse)
        let content_length = header_str
            .lines()
            .filter_map(|line| {
                let lower = line.trim().to_ascii_lowercase();
                if let Some(stripped) = lower.strip_prefix("content-length:") {
                    stripped.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .next()?;

        if body_start + content_length > self.buf.len() {
            return None; // pas assez de données
        }

        let payload = self.buf[body_start..body_start + content_length].to_vec();

        // Supprimer le frame lu du buffer
        self.buf.drain(..body_start + content_length);

        Some(payload)
    }
}

/// Identifiant d'un client abonné à une session LSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

// ── Session interne ──────────────────────────────────────────────────────────

/// Implémentation interne de la session LSP : process, multiplexage, communication.
struct LspSessionInner {
    cmd_tx: mpsc::Sender<Vec<u8>>,
    /// session_id -> (client who sent the request, original JSON-RPC id)
    pending: Mutex<HashMap<u64, (ClientId, i64)>>,
    subs: Mutex<HashMap<ClientId, mpsc::UnboundedSender<Vec<u8>>>>,
    alive: AtomicBool,
    child: Mutex<Option<Child>>,
    next_client: AtomicU64,
    next_req: AtomicU64,
    /// `true` dès qu'un client a envoyé `initialize` au process (partagé entre tous
    /// les clients de la session — le process LSP ne s'initialise qu'une fois).
    initialized: AtomicBool,
    /// Issue de la réponse `initialize` réelle, posée par le premier client (celui
    /// qui a gagné `try_mark_initialized`) une fois sa réponse reçue — `Ok(result)`
    /// ou `Err(error)`. Les clients suivants (nouvel onglet, rechargement de page,
    /// nouveau tool MCP) doivent recevoir une réponse `initialize` sans renvoyer la
    /// requête au process — un LSP réel rejette un second `initialize` (violation du
    /// protocole ; observé en usage réel : rust-analyzer répond `-32601 unknown
    /// request`). Cache aussi l'ÉCHEC (pas seulement le succès) : sans ça, un premier
    /// `initialize` en échec (ex. typescript-language-server sans `node_modules`
    /// local, observé en usage réel) laisse `initialized` à `true` pour toujours sans
    /// rien à rejouer — tout client suivant attendrait le timeout de 30s pour rien
    /// plutôt que de recevoir immédiatement la même erreur réelle.
    initialize_outcome: Mutex<Option<Result<Value, Value>>>,
    initialize_notify: Notify,
    /// URIs pour lesquels un `didOpen` a déjà été envoyé au process, partagé entre
    /// tous les clients (éditeur navigateur, chaque appel de tool MCP — un nouveau
    /// `LspClient` par appel). Un LSP réel n'attend `didOpen` qu'une fois par URI tant
    /// que rien ne l'a fermé (`didClose`) — un second `didOpen` sur une URI déjà
    /// ouverte est une violation de protocole. Observé en usage réel comme cause
    /// probable de `lsp_diagnostics` "one-shot" (diagnostics présents au premier
    /// appel, absents ensuite) : chaque appel MCP renvoyait son propre `didOpen` sur
    /// le même fichier.
    open_uris: Mutex<HashSet<String>>,
    /// Version de doc par URI pour les didChange émis par les TOOLS (cas A). Les
    /// compteurs navigateur (@codemirror/lsp-client) sont indépendants et ne
    /// croisent jamais les nôtres : en cas B le tool n'envoie JAMAIS didChange
    /// (design R1) — deux émetteurs actifs sur la même URI = désync, interdit.
    doc_versions: Mutex<HashMap<String, i32>>,
    /// URIs tenues par au moins un client navigateur, par ClientId (piste R1 sq1).
    /// Alimenté par le bridge ws/lsp.rs SEULEMENT (subscribe du bridge = client
    /// navigateur ; les LspClient des tools ne s'enregistrent pas ici). Nettoyé
    /// sur didClose navigateur et sur unsubscribe (déconnexion = plus de tenant).
    editor_uris: Mutex<HashMap<String, HashSet<ClientId>>>,
    /// Dernier `publishDiagnostics.diagnostics` connu par URI, alimenté par la tâche
    /// lectrice pour TOUTE notification reçue — indépendamment de qui est abonné à ce
    /// moment (cf. `wait_for_diagnostics`/`cached_diagnostics`). Nécessaire : un
    /// `LspClient` MCP s'abonne fraîchement à chaque appel de tool ; sans ce cache, un
    /// push déjà arrivé avant cet abonnement (ex. diagnostics publiés juste après
    /// l'ouverture du fichier par l'éditeur navigateur, avant qu'un tool MCP ne
    /// s'y intéresse) est perdu pour ce client — observé en usage réel comme cause
    /// probable de "jamais de diagnostics Rust" alors que l'éditeur navigateur les
    /// voit bien. Alimenté indépendamment du fait que `textDocument/diagnostic`
    /// (pull) soit supporté ou non — vérifié : rust-analyzer le supporte,
    /// typescript-language-server non (`"Unhandled method"`) — donc pas de solution
    /// pull uniforme entre les deux, le cache push est la seule option qui marche
    /// pour les deux serveurs.
    diagnostics_cache: Mutex<HashMap<String, Vec<Value>>>,
    diagnostics_notify: Notify,
    /// Enfants auxiliaires du multiplexeur (`spec.aux` spawnés, rôles connus).
    /// Vide ⟹ chemin mono-process strictement inchangé (invariant 8) : le
    /// primaire n'est PAS dans cette liste — il garde ses champs historiques
    /// ci-dessus (`cmd_tx`, `pending`, `next_req`, `child`).
    aux: Vec<Arc<AuxChild>>,
    _toolchain_name: String,
}

/// Contexte d'une requête émise par la session vers un aux (ids internes —
/// leurs réponses ne sont JAMAIS routées à un client).
enum InternalPending {
    /// Copie d'`initialize` (task-03) : succès → rien, erreur → warn dégradé.
    Initialize,
    /// `workspace/executeCommand typescript.tsserverRequest` émis en réponse à
    /// un `tsserver/request` du primaire : `vue_id` est l'élément [0] de
    /// `params[0]`, restitué TEL QUEL dans la `tsserver/response`.
    TsserverForward { vue_id: Value },
}

/// Un enfant auxiliaire du multiplexeur (jamais le primaire). Possède son canal
/// de stdin (fan-out des notifications, copie de `initialize` avec ids
/// internes), son flag de vie (mort d'un aux = session dégradée, pas morte —
/// `is_alive()` ne reflète que le primaire), son compteur d'ids internes et
/// la map de ces ids en attente avec leur contexte (`InternalPending` — leurs
/// réponses sont interceptées par la session, JAMAIS routées à un client), et
/// sa handle pour kill.
struct AuxChild {
    /// `LspAux::role` — tracage et messages de dégradation.
    role: String,
    /// `initOptions` du spec, fusionnés (clés gagnantes) dans
    /// `params.initializationOptions` du `initialize` envoyé à CET enfant.
    init_options: Value,
    stdin_tx: mpsc::Sender<Vec<u8>>,
    alive: AtomicBool,
    next_internal_req: AtomicU64,
    pending_internal: Mutex<HashMap<u64, InternalPending>>,
    child: Mutex<Option<Child>>,
}

/// Intermède interne à `LspSession::spawn` : process aux spawné (pré-pass) mais
/// tâches pas encore lancées — elles nécessitent le `Arc<LspSession>` (la
/// lectrice aux y dispatch notifications et dégradations).
struct AuxStartup {
    handle: Arc<AuxChild>,
    stdin: ChildStdin,
    stdin_rx: mpsc::Receiver<Vec<u8>>,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

/// Tâche écrivaine partagée par tous les enfants (primaire et aux) : canal →
/// stdin encodé `Content-Length`.
fn spawn_stdin_writer(mut stdin: ChildStdin, mut cmd_rx: mpsc::Receiver<Vec<u8>>) {
    tokio::spawn(async move {
        while let Some(data) = cmd_rx.recv().await {
            let encoded = encode_message(&data);
            if stdin.write_all(&encoded).await.is_err() {
                break;
            }
            if stdin.flush().await.is_err() {
                break;
            }
        }
        // Canal fermé → le process voit EOF stdin
    });
}

/// Spawn du process d'un aux : mêmes conditions que le primaire (`cwd =
/// sandbox_root`, stdio pipés) + canal de stdin dédié. Les tâches
/// lectrice/écrivaine/stderr sont lancées séparément par `LspSession::spawn`
/// une fois la session Arc constituée.
async fn spawn_aux_startup(aux_spec: &LspAux, sandbox_root: &Path) -> anyhow::Result<AuxStartup> {
    let mut cmd = Command::new(&aux_spec.bin);
    cmd.args(&aux_spec.args)
        .current_dir(sandbox_root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("aux spawn error: {e}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing stderr"))?;

    let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    let handle = Arc::new(AuxChild {
        role: aux_spec.role.clone(),
        init_options: aux_spec.init_options.clone(),
        stdin_tx,
        alive: AtomicBool::new(true),
        next_internal_req: AtomicU64::new(1),
        pending_internal: Mutex::new(HashMap::new()),
        child: Mutex::new(Some(child)),
    });
    Ok(AuxStartup {
        handle,
        stdin,
        stdin_rx,
        stdout,
        stderr,
    })
}

/// `initializationOptions` à poser sur la copie d'`initialize` envoyée à un aux.
/// La copie de la requête client étant déjà verbatim, cette fonction ne calcule
/// une valeur que quand le spec a des `initOptions` (objet) à injecter : objet
/// du client fusionné puis clés du spec (spec gagnant ; `plugins` est un
/// tableau, pas de merge profond à faire ici), et le spec verbatim si le client
/// n'avait pas d'options. `None` = rien à injecter (la copie garde l'état du
/// client tel quel).
fn merged_initialization_options(req: &Value, spec_opts: &Value) -> Option<Value> {
    let spec_obj = spec_opts.as_object()?;
    let mut merged = req
        .get("params")
        .and_then(|p| p.get("initializationOptions"))
        .and_then(|opts| opts.as_object())
        .cloned()
        .unwrap_or_default();
    for (key, value) in spec_obj {
        merged.insert(key.clone(), value.clone());
    }
    Some(Value::Object(merged))
}

/// Extrait `(vue_id, command, payload)` d'une notification `tsserver/request`
/// du primaire : `params` est un tableau dont `params[0]` est le tableau
/// `[id, commande, payload?]`. `None` = trame malformée (commande absente ou
/// non-string, params pas `[ […] ]`…) — `vue_id` alors non exploitable, on ne
/// répond pas. `payload` absent ou `null` ⟹ `Value::Null` (passé tel quel
/// dans les `arguments` de l'`executeCommand`).
fn parse_tsserver_request(msg: &Value) -> Option<(Value, &str, Value)> {
    let inner = msg.get("params")?.get(0)?.as_array()?;
    let command = inner.get(1)?.as_str()?;
    let vue_id = inner.first()?.clone();
    let payload = inner.get(2).cloned().unwrap_or(Value::Null);
    Some((vue_id, command, payload))
}

/// Session LSP : possède un process, multiplexe les clients.
pub struct LspSession {
    inner: Arc<LspSessionInner>,
}

impl LspSession {
    /// Spawn le process primaire (`spec.bin` + `spec.args`) avec `cwd =
    /// sandbox_root`, stdio pipé (stdin/stdout), stderr pipé et loggé via
    /// `tracing`. Lance les tâches lectrice (stdout → FrameReader → dispatch) et
    /// écrivaine (canal → stdin encodé). Erreur si le spawn du PRIMAIRE échoue ou
    /// si stdin/stdout/stderr sont absents.
    ///
    /// Toolchain composite (`spec.aux` non vide) : chaque aux de rôle connu est
    /// spawné aux mêmes conditions et reçoit le fan-out des notifications et une
    /// copie de `initialize` avec `initOptions` injectés (cf. `AuxChild`). Spawn
    /// d'aux en échec ou rôle inconnu = session dégradée (`tracing::warn!`),
    /// jamais une erreur de session (invariant 1). `spec.aux` vide ⟹ chemin
    /// mono-process strictement actuel (invariant 8 — tout le LSP déployé dépend
    /// de cette non-régression).
    pub async fn spawn(spec: &LspToolchain, sandbox_root: &Path) -> anyhow::Result<Arc<Self>> {
        let mut cmd = Command::new(&spec.bin);
        cmd.args(&spec.args)
            .current_dir(sandbox_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("LSP spawn error for {}: {e}", spec.name))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdout"))?;
        let stderr_handle = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stderr"))?;
        // child handle still valid for kill

        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        // ── Enfants auxiliaires du multiplexeur — pré-pass processus seulement
        // (les tâches démarrent une fois le session Arc constitué, chaque
        // lectrice aux y dispatchant trames et dégradations). Rôle inconnu :
        // warn + enfant ignoré ; spawn en échec : warn « aux degraded ». Ni l'un
        // ni l'autre ne fait échouer le spawn de la session (invariant 1).
        let mut aux_startups: Vec<AuxStartup> = Vec::new();
        for aux_spec in &spec.aux {
            if !is_known_aux_role(&aux_spec.role) {
                tracing::warn!(
                    toolchain = spec.name.as_str(),
                    role = aux_spec.role.as_str(),
                    "LSP: unknown aux role, aux ignored"
                );
                continue;
            }
            match spawn_aux_startup(aux_spec, sandbox_root).await {
                Ok(startup) => aux_startups.push(startup),
                Err(e) => {
                    tracing::warn!(
                        toolchain = spec.name.as_str(),
                        role = aux_spec.role.as_str(),
                        "LSP: aux degraded (spawn failed): {e}"
                    );
                }
            }
        }
        let aux: Vec<Arc<AuxChild>> = aux_startups
            .iter()
            .map(|startup| Arc::clone(&startup.handle))
            .collect();

        let session = Arc::new(LspSession {
            inner: Arc::new(LspSessionInner {
                cmd_tx,
                pending: Mutex::new(HashMap::new()),
                subs: Mutex::new(HashMap::new()),
                alive: AtomicBool::new(true),
                initialized: AtomicBool::new(false),
                initialize_outcome: Mutex::new(None),
                initialize_notify: Notify::new(),
                open_uris: Mutex::new(HashSet::new()),
                doc_versions: Mutex::new(HashMap::new()),
                editor_uris: Mutex::new(HashMap::new()),
                diagnostics_cache: Mutex::new(HashMap::new()),
                diagnostics_notify: Notify::new(),
                child: Mutex::new(Some(child)),
                next_client: AtomicU64::new(1),
                next_req: AtomicU64::new(1),
                aux,
                _toolchain_name: spec.name.clone(),
            }),
        });

        // Écrivaine : canal → stdin encodé
        spawn_stdin_writer(stdin, cmd_rx);

        // Lectrice : stdout → FrameReader → dispatch
        let reader_session = Arc::clone(&session);
        tokio::spawn(async move {
            let mut stdout = stdout;
            let mut frame_reader = FrameReader::new();
            let mut buf = [0u8; 8192];

            loop {
                let n = match stdout.read(&mut buf).await {
                    Ok(0) => break, // EOF → process mort
                    Ok(n) => n,
                    Err(_) => break,
                };

                frame_reader.push(&buf[..n]);
                while let Some(payload) = frame_reader.next_frame() {
                    // Trame non-JSON → ignored with warn
                    let msg: Value = match serde_json::from_slice(&payload) {
                        Ok(msg) => msg,
                        Err(_) => {
                            tracing::warn!(
                                toolchain = reader_session.inner._toolchain_name.as_str(),
                                "LSP: non-JSON frame, ignoring"
                            );
                            continue;
                        }
                    };

                    // Trame avec `id` → réponse : router au client d'origine
                    if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                        let session_id = id as u64;
                        let mut pending = match reader_session.inner.pending.lock() {
                            Ok(g) => g,
                            Err(g) => g.into_inner(),
                        };

                        if let Some((client, orig_id)) = pending.remove(&session_id) {
                            // Restaurer l'original id dans la réponse
                            let mut outbound = msg.clone();
                            outbound["id"] = Value::Number(serde_json::Number::from(orig_id));
                            // Envoi au client : JSON brut (pas de framing — le client n'utilise pas FrameReader)
                            if let Ok(subs) = reader_session.inner.subs.lock()
                                && let Some(tx) = subs.get(&client)
                            {
                                let _ = tx.send(outbound.to_string().into_bytes());
                            }
                        } else {
                            tracing::warn!(
                                session_id,
                                "LSP: response for unknown session_id, ignoring"
                            );
                        }
                    } else {
                        // Notification (pas d'id). `tsserver/request` (que
                        // vue-language-server en mode hybride v3 envoie à SON
                        // client primaire pour une exécution tsserver) est
                        // INTERCEPTÉE ici — jamais broadcast aux clients
                        // (tâche 05) ; toute autre notification : chemin
                        // actuel — cache `publishDiagnostics` puis broadcast.
                        if msg.get("method").and_then(|m| m.as_str()) == Some("tsserver/request") {
                            reader_session.handle_tsserver_request(&msg);
                        } else {
                            reader_session.dispatch_notification_frame(&msg, payload);
                        }
                    }
                }
            }

            // EOF stdout → process mort
            reader_session.inner.alive.store(false, Ordering::SeqCst);
            if let Ok(mut subs) = reader_session.inner.subs.lock() {
                subs.clear();
            }
            // Invariant 6 : mort du primaire = mort de la session ⟹ kill
            // explicite des enfants aux. Flag alive baissé avant kill : leur
            // EOF stdout ne doit pas sonner « degraded » alors que la session
            // est déjà morte — ce n'est pas une dégradation, c'est la mort.
            for aux in &reader_session.inner.aux {
                aux.alive.store(false, Ordering::SeqCst);
                if let Some(mut child) = aux.child.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    drop(child.kill());
                }
            }
        });

        // Stderr logger : stderr → tracing
        let _stderr_session = Arc::clone(&session);
        tokio::spawn(async move {
            let mut stderr = stderr_handle;
            let mut buf = [0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let line = String::from_utf8_lossy(&buf[..n]);
                tracing::debug!(
                    toolchain = _stderr_session.inner._toolchain_name.as_str(),
                    "LSP stderr: {}",
                    line.trim()
                );
            }
        });

        // ── Tâches des aux (session Arc constitué) : écrivaine, lectrice,
        // stderr.
        for startup in aux_startups {
            let AuxStartup {
                handle: aux,
                stdin: aux_stdin,
                stdin_rx: aux_rx,
                stdout: aux_stdout,
                stderr: aux_stderr,
            } = startup;

            spawn_stdin_writer(aux_stdin, aux_rx);

            // Lectrice aux : distingue par ENFANT (cette tâche EST l'enfant).
            // Les ids internes de l'aux ne sont JAMAIS routés à un client :
            // réponse à un id interne → interceptée par la session et
            // dispatchée par variante d'`InternalPending` (`Initialize` :
            // succès → rien, erreur → warn « aux degraded » ; `TsserverForward`
            // → body dépilé et `tsserver/response` au primaire), toute autre
            // trame à `id` (server→aux request, id sans correspondance) →
            // warn + drop, comme le pending miss du chemin primaire.
            let reader_aux = Arc::clone(&aux);
            let reader_session = Arc::clone(&session);
            tokio::spawn(async move {
                let mut stdout = aux_stdout;
                let mut frame_reader = FrameReader::new();
                let mut buf = [0u8; 8192];

                loop {
                    let n = match stdout.read(&mut buf).await {
                        Ok(0) => break, // EOF → aux mort : dégradation, pas mort de session
                        Ok(n) => n,
                        Err(_) => break,
                    };

                    frame_reader.push(&buf[..n]);
                    while let Some(payload) = frame_reader.next_frame() {
                        let msg: Value = match serde_json::from_slice(&payload) {
                            Ok(msg) => msg,
                            Err(_) => {
                                tracing::warn!(
                                    toolchain = reader_session.inner._toolchain_name.as_str(),
                                    role = reader_aux.role.as_str(),
                                    "LSP: non-JSON frame from aux, ignoring"
                                );
                                continue;
                            }
                        };

                        if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                            let pending_entry = {
                                let mut pending = match reader_aux.pending_internal.lock() {
                                    Ok(g) => g,
                                    Err(g) => g.into_inner(),
                                };
                                pending.remove(&(id as u64))
                            };
                            match pending_entry {
                                Some(InternalPending::Initialize) => {
                                    // Réponse à l'`initialize` interne — jamais
                                    // relayée au client (invariant 3) : succès →
                                    // rien, erreur → warn dégradé.
                                    if let Some(error) = msg.get("error") {
                                        tracing::warn!(
                                            toolchain =
                                                reader_session.inner._toolchain_name.as_str(),
                                            role = reader_aux.role.as_str(),
                                            "LSP: aux degraded (initialize error): {error}"
                                        );
                                    }
                                }
                                Some(InternalPending::TsserverForward { vue_id }) => {
                                    // Contrat tâche 05 : le résultat LSP de
                                    // `typescript.tsserverRequest` est l'OBJET
                                    // RÉPONSE TSSERVER COMPLET — on en dépile
                                    // `body`. Tout ce qui n'est pas un body
                                    // exploitable (résultat absent — erreur
                                    // JSON-RPC — , `null`/sentinelle
                                    // NoContent, sans champ `body`, non-objet)
                                    // ⟹ body `null`.
                                    if let Some(error) = msg.get("error") {
                                        tracing::warn!(
                                            toolchain =
                                                reader_session.inner._toolchain_name.as_str(),
                                            role = reader_aux.role.as_str(),
                                            "LSP: aux degraded (tsserver forward error): {error}"
                                        );
                                    }
                                    let body = msg
                                        .get("result")
                                        .and_then(|r| r.get("body"))
                                        .cloned()
                                        .unwrap_or(Value::Null);
                                    reader_session.send_tsserver_response(vue_id, body);
                                }
                                None => {
                                    tracing::warn!(
                                        toolchain = reader_session.inner._toolchain_name.as_str(),
                                        role = reader_aux.role.as_str(),
                                        session_id = id,
                                        "LSP: response from aux for unknown internal id, dropping"
                                    );
                                }
                            }
                        } else {
                            // Notification d'un enfant (aux inclus) : chemin
                            // actuel — cache `publishDiagnostics` (last-writer-
                            // wins, TODO(task-06) porté par
                            // `dispatch_notification_frame`) puis broadcast.
                            reader_session.dispatch_notification_frame(&msg, payload);
                        }
                    }
                }

                // EOF stdout d'un AUX : la session SURVIT (`is_alive()` ne
                // reflète que le primaire). Aux baissé, ids internes en
                // attente purgés (ils n'auront jamais de réponse) — les
                // `TsserverForward` reçoivent d'abord une `tsserver/response`
                // `[[vue_id, null]]` (best effort `try_send` : primaire mort ⟹
                // ignoré, la requête d'origine n'aurait de toute façon jamais
                // existé) pour que `vue-language-server` ne pende pas ; les
                // `Initialize` se font clear comme avant (tâche 03) — et warn
                // unique « degraded » seulement si la session est encore
                // vivante : si le primaire vient de mourir en tuant cet aux,
                // la dégradation serait du bruit.
                reader_aux.alive.store(false, Ordering::SeqCst);
                {
                    let mut pending = reader_aux
                        .pending_internal
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for entry in pending.values() {
                        if let InternalPending::TsserverForward { vue_id } = entry {
                            reader_session.send_tsserver_response(vue_id.clone(), Value::Null);
                        }
                    }
                    pending.clear();
                }
                if reader_session.inner.alive.load(Ordering::SeqCst) {
                    tracing::warn!(
                        toolchain = reader_session.inner._toolchain_name.as_str(),
                        role = reader_aux.role.as_str(),
                        "LSP: aux degraded (stdout EOF)"
                    );
                }
            });

            // Stderr de l'aux → tracing::debug! (comme le primaire, role en plus)
            let stderr_aux = Arc::clone(&aux);
            let stderr_session = Arc::clone(&session);
            tokio::spawn(async move {
                let mut stderr = aux_stderr;
                let mut buf = [0u8; 8192];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf[..n]);
                    tracing::debug!(
                        toolchain = stderr_session.inner._toolchain_name.as_str(),
                        role = stderr_aux.role.as_str(),
                        "LSP stderr (aux): {}",
                        line.trim()
                    );
                }
            });
        }

        Ok(session)
    }

    /// Traite une trame NOTIFICATION (sans `id`) reçue d'un enfant — primaire ou
    /// aux — chemin partagé par invariant 5 : cache `publishDiagnostics` pour
    /// TOUT abonné, présent ou futur — indépendant du broadcast ci-dessous (cf.
    /// doc du champ `diagnostics_cache`) — puis broadcast aux abonnés (nettoyage
    /// des canaux fermés au passage).
    ///
    /// TODO(task-06) : la fusion des `publishDiagnostics` entre enfants (concat
    /// par URI) se fera AVANT l'écriture du cache — navigateur et MCP doivent
    /// voir le même résultat fondu (contrainte, pas option). En attendant :
    /// last-writer-wins (comportement mono-process actuel, `aux: []` ⟹ seule la
    /// lectrice primaire alimente ce chemin, donc strictement inchangé).
    fn dispatch_notification_frame(&self, msg: &Value, payload: Vec<u8>) {
        // `publishDiagnostics` : cache + notify (chemin actuel).
        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics")
            && let Some(params) = msg.get("params")
            && let Some(uri) = params.get("uri").and_then(|u| u.as_str())
            && let Some(diags) = params.get("diagnostics").and_then(|d| d.as_array())
        {
            if let Ok(mut cache) = self.inner.diagnostics_cache.lock() {
                cache.insert(uri.to_string(), diags.clone());
            }
            self.inner.diagnostics_notify.notify_waiters();
        }

        // Notification (pas d'id) : broadcast à tous les abonnés
        let subs_map = match self.inner.subs.lock() {
            Ok(g) => g,
            Err(g) => g.into_inner(),
        };

        let dead_clients: Vec<_> = subs_map
            .iter()
            .filter(|(_client_id, tx)| {
                let raw_json = payload.clone();
                if tx.send(raw_json).is_err() {
                    return true; // client mort (canal fermé)
                }
                false
            })
            .map(|(id, _)| *id)
            .collect();

        // Nettoyer les clients morts
        if !dead_clients.is_empty()
            && let Ok(mut subs) = self.inner.subs.lock()
        {
            for client_id in dead_clients {
                subs.remove(&client_id);
            }
        }
    }

    /// Invariant 3 : copie de l'`initialize` client vers chaque aux VIVANT, avec
    /// un id interne propre à l'enfant (compteur par enfant — l'espace d'ids du
    /// primaire n'est jamais touché) et `params.initializationOptions` fusionné
    /// (`merged_initialization_options`). La réponse est interceptée par la
    /// lectrice aux (jamais routée à un client). Ne rend jamais d'erreur : un aux
    /// qui ne peut rien recevoir se dégrade lui-même (`debug`) — il ne peut ni
    /// bloquer ni faire échouer la réponse du primaire.
    fn fan_out_initialize(&self, msg: &Value) {
        for aux in &self.inner.aux {
            if !aux.alive.load(Ordering::SeqCst) {
                continue; // déjà dégradé : ignoré silencieusement
            }
            let internal_id = aux.next_internal_req.fetch_add(1, Ordering::SeqCst);
            let mut copy = msg.clone();
            copy["id"] = Value::Number(serde_json::Number::from(internal_id));
            if let Some(merged) = merged_initialization_options(msg, &aux.init_options) {
                copy["params"]["initializationOptions"] = merged;
            }
            // Enregistrement AVANT l'envoi : évite la course avec la lectrice
            // aux qui verrait une réponse sans correspondance (warn parasite).
            aux.pending_internal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(internal_id, InternalPending::Initialize);
            // `try_send` (pas `send().await`) : même raison que les
            // notifications — un aux bloqué ou mort ne doit jamais bloquer ni
            // faire échouer le chemin client.
            if aux
                .stdin_tx
                .try_send(copy.to_string().into_bytes())
                .is_err()
            {
                aux.pending_internal
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&internal_id);
                tracing::debug!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    role = aux.role.as_str(),
                    "LSP: aux degraded (cannot receive initialize)"
                );
            }
        }
    }

    /// Intercepte une notification `tsserver/request` émise par le PRIMAIRE
    /// (mode hybride Volar v3 : `vue-language-server` ne parle pas à tsserver
    /// seul et délègue à SON client — ici la session). Exécution contre le
    /// premier aux `tsserver-forward` VIVANT via `workspace/executeCommand
    /// typescript.tsserverRequest` (`arguments = [command, payload]`
    /// exactement — le 3e argument `ExecuteInfo`, optionnel sur TLS 6.0.0,
    /// n'est PAS envoyé : ses defaults conviennent). La réponse de l'aux
    /// (objet tsserver complet) est dépilée par la lectrice aux et renvoyée
    /// en `tsserver/response` — JAMAIS un broadcast aux clients (dialogue
    /// interne, le navigateur n'en veut pas).
    ///
    /// Aucun aux vivant du rôle (absent, mort, ou canal `try_send` plein) ⟹
    /// réponse `[[vue_id, null]]` IMMÉDIATE : `vue-language-server` ne pend
    /// jamais faute d'aux. Trame malformée ⟹ `warn` + drop, sans réponse (le
    /// `vue_id` n'est pas exploitable) — jamais une panique ni une erreur de
    /// session.
    ///
    /// NB (`aux: []`, mono-process) : un primaire inconnu qui émettrait quand
    /// même `tsserver/request` (personne aujourd'hui hors composite Volar)
    /// voit sa trame INTERCEPTÉE — plus de broadcast aux clients — puis
    /// `[[id, null]]` renvoyé : nouveau comportement assumé par la tâche 05.
    fn handle_tsserver_request(&self, msg: &Value) {
        let (vue_id, command, payload) = match parse_tsserver_request(msg) {
            Some(parsed) => parsed,
            None => {
                tracing::warn!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    "LSP: malformed tsserver/request from primary, dropping"
                );
                return;
            }
        };
        let alive_aux = self
            .inner
            .aux
            .iter()
            .find(|aux| aux.role == "tsserver-forward" && aux.alive.load(Ordering::SeqCst));
        let Some(aux) = alive_aux else {
            // Aucun aux vivant du rôle ⟹ body null immédiat (primaire vivant
            // ou déjà mort — `try_send` err alors, sans conséquence).
            self.send_tsserver_response(vue_id, Value::Null);
            return;
        };
        let internal_id = aux.next_internal_req.fetch_add(1, Ordering::SeqCst);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": internal_id,
            "method": "workspace/executeCommand",
            "params": {
                "command": "typescript.tsserverRequest",
                "arguments": [command, payload],
            }
        });
        // Enregistrement AVANT envoi (même raison que `fan_out_initialize` :
        // éviter que la lectrice aux voie une réponse sans correspondance).
        aux.pending_internal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                internal_id,
                InternalPending::TsserverForward {
                    vue_id: vue_id.clone(),
                },
            );
        if aux
            .stdin_tx
            .try_send(request.to_string().into_bytes())
            .is_err()
        {
            // Aux marqué vivant mais injoignable (canal plein ou fermé) : il
            // ne répondra jamais ⟹ dégradation + body null immédiat.
            aux.pending_internal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&internal_id);
            self.send_tsserver_response(vue_id, Value::Null);
        }
    }

    /// `tsserver/response` de la session vers le PRIMAIRE (sa stdin via
    /// `cmd_tx`) : notification JSON-RPC `params = [[vue_id, body]]`, `vue_id`
    /// restitué TEL QUEL (numérique ou string — le primaire est seul juge de
    /// ses ids). Jamais une notification client ; `cmd_tx` fermé ⟹ primaire
    /// mort, la réponse n'a plus de destinataire : ignorée silencieusement.
    fn send_tsserver_response(&self, vue_id: Value, body: Value) {
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tsserver/response",
            "params": [[vue_id, body]],
        });
        let _ = self.inner.cmd_tx.try_send(frame.to_string().into_bytes());
    }

    /// Abonne un client. Rend `(ClientId, UnboundedReceiver<Vec<u8>>)`.
    /// Le receiver reçoit les réponses (id restauré) et notifications serveur.
    /// `None` quand le process meurt (canal fermé par la tâche lectrice).
    pub fn subscribe(&self) -> (ClientId, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let client_id = ClientId(self.inner.next_client.fetch_add(1, Ordering::SeqCst));
        if let Ok(mut subs) = self.inner.subs.lock() {
            subs.insert(client_id, tx);
        }
        (client_id, rx)
    }

    /// Désabonne un client : retire le canal de sortie, les entrées `pending`
    /// de ce client et sa présence dans toutes les sets de `editor_uris`
    /// (déconnexion = plus de tenant, piste R1 sq1 — sans ça, une URI fermée
    /// brutalement (WS coupé, pas de `didClose`) resterait tenue pour toujours).
    /// Les `LspClient` des tools passent aussi par ici (leur `Drop`) mais ne
    /// sont jamais dans `editor_uris` — le nettoyage est un no-op pour eux.
    pub fn unsubscribe(&self, client: ClientId) {
        if let Ok(mut subs) = self.inner.subs.lock() {
            subs.remove(&client);
        }
        // Retirer les pending requests de ce client
        let mut pending = match self.inner.pending.lock() {
            Ok(g) => g,
            Err(g) => g.into_inner(),
        };
        pending.retain(|_, (c, _)| *c != client);
        // Retirer le client de toutes les sets d'URI tenues par des éditeurs ;
        // une URI sans aucun tenant disparaît de la map.
        let mut editor_uris = self
            .inner
            .editor_uris
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for clients in editor_uris.values_mut() {
            clients.remove(&client);
        }
        editor_uris.retain(|_, clients| !clients.is_empty());
    }

    /// Envoie un message JSON-RPC client → enfant(s).
    ///
    /// Multiplexeur : requête avec `id` → primaire uniquement, CHEMIN INCHANGÉ
    /// (routage/fusion par politique : tâches 04–06), sauf `initialize` dont une
    /// copie avec `initOptions` injectés part aussi à chaque aux vivant
    /// (`fan_out_initialize`, invariable 3). Trame sans `id` (notification,
    /// doc-sync inclus) → primaire puis fan-out telle quelle à chaque aux
    /// vivant (invariant 2) — un aux mort ou bloqué est ignoré, ne bloque ni
    /// n'erre jamais le chemin client. `aux` vide ⟹ chemin mono-process
    /// strictement actuel (invariant 8).
    ///
    /// Erreurs :
    /// - JSON invalide → `VNL-SBX-LSP-001`
    /// - `id` présent mais non entier → `VNL-SBX-LSP-002`
    /// - process fermé (canal `cmd_tx` fermé) → `VNL-SBX-LSP-003`
    pub async fn send(&self, client: ClientId, payload: Vec<u8>) -> anyhow::Result<()> {
        // Valider JSON
        let msg: Value = serde_json::from_slice(&payload)
            .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-001: invalid JSON payload"))?;

        // Vérifier l'`id` : doit être un entier (i64) si présent
        if let Some(id_val) = msg.get("id")
            && id_val.as_i64().is_none()
        {
            return Err(anyhow::anyhow!(
                "VNL-SBX-LSP-002: JSON-RPC id must be an integer, got non-integer"
            ));
        }

        if msg.get("id").is_some() {
            // Requête : réécrire l'id en session id et mémoriser le mapping
            let session_req_id = self.inner.next_req.fetch_add(1, Ordering::SeqCst);
            let orig_id = msg["id"].as_i64().unwrap_or(0);

            // Réécrire l'id dans le payload pour le processus
            let mut rewritten = msg.clone();
            rewritten["id"] = Value::Number(serde_json::Number::from(session_req_id));
            let rewritten_payload = rewritten.to_string().into_bytes();

            // Mémoriser le mapping session_id -> (client, orig_id)
            if let Ok(mut pending) = self.inner.pending.lock() {
                pending.insert(session_req_id, (client, orig_id));
            }

            // Envoyer au process
            self.inner
                .cmd_tx
                .send(rewritten_payload)
                .await
                .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"))?;

            // Invariant 3 : `initialize` part AUSSI en copie (id interne propre,
            // `initOptions` du spec injectés) vers chaque aux vivant. Les autres
            // requêtes restent primaire uniquement — routage par politique de
            // fusion et remapping d'ids par enfant : tâches 04–06, NE RIEN
            // fusionner ici.
            if msg.get("method").and_then(|m| m.as_str()) == Some("initialize") {
                self.fan_out_initialize(&msg);
            }
            Ok(())
        } else {
            // Notification : transmettre tel quel au primaire (chemin actuel)…
            if self.inner.aux.is_empty() {
                return self
                    .inner
                    .cmd_tx
                    .send(payload)
                    .await
                    .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"));
            }
            self.inner
                .cmd_tx
                .send(payload.clone())
                .await
                .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"))?;
            // …puis fan-out à chaque enfant aux VIVANT (invariant 2). Enfant mort
            // ignoré silencieusement (déjà dégradé) ; `try_send` par design : un
            // aux bloqué ne doit jamais suspendre le chemin client — une trame
            // perdue pour un aux saturé fait partie de sa dégradation, jamais de
            // celle du primaire.
            for aux in &self.inner.aux {
                if !aux.alive.load(Ordering::SeqCst) {
                    continue;
                }
                let _ = aux.stdin_tx.try_send(payload.clone());
            }
            Ok(())
        }
    }

    /// `true` tant que le process n'a pas rendu EOF stdout.
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }

    /// `true` si le process a déjà reçu `initialize`.
    pub fn is_initialized(&self) -> bool {
        self.inner.initialized.load(Ordering::SeqCst)
    }

    /// Test-and-set : rend `true` si CE client gagne le droit d'envoyer `initialize`
    /// (le flag était à `false`), `false` si un autre client l'a déjà initialisé.
    pub fn try_mark_initialized(&self) -> bool {
        !self.inner.initialized.swap(true, Ordering::SeqCst)
    }

    /// Pose l'issue de la réponse `initialize` réelle — `Ok(result)` en cas de
    /// succès, `Err(error)` si le process a répondu une erreur (ex.
    /// typescript-language-server sans `node_modules` local trouvable) — appelé par
    /// le client gagnant de `try_mark_initialized` une fois sa réponse reçue, et
    /// réveille les clients en attente dans `wait_for_initialize_outcome`. Un échec
    /// EST mis en cache, pas seulement un succès : sans ça, tout client suivant
    /// attendrait le timeout de 30s pour rien plutôt que de recevoir immédiatement la
    /// même erreur réelle (bug trouvé en usage réel).
    pub fn set_initialize_outcome(&self, outcome: Result<Value, Value>) {
        if let Ok(mut guard) = self.inner.initialize_outcome.lock() {
            *guard = Some(outcome);
        }
        self.inner.initialize_notify.notify_waiters();
    }

    /// Rend l'issue `initialize` mise en cache dès qu'elle est disponible — pour un
    /// client qui a perdu `try_mark_initialized` (déjà posée : retour immédiat ; pas
    /// encore posée : attend `notify_waiters`, borné à 30s au cas où le client
    /// gagnant n'aboutit jamais — process tué avant toute réponse, par ex.). `None`
    /// seulement dans ce cas de timeout réel.
    ///
    /// Course bénigne assumée : `Notify::notify_waiters` (contrairement à
    /// `notify_one`) ne mémorise pas de "permit" — un appelant qui n'a pas encore
    /// atteint le `select!` ci-dessous au moment de l'appel à `set_initialize_outcome`
    /// peut manquer le réveil. Sans conséquence sur l'exactitude (le re-check du
    /// cache après le `select!` couvre ce cas), seulement sur la latence dans cette
    /// fenêtre étroite : au pire les 30s complètes avant de relire un cache déjà
    /// peuplé, plutôt qu'un réveil immédiat. Pas de mécanisme plus strict
    /// (`notify_one` + compteur, boucle de poll courte) pour une fenêtre de course
    /// aussi étroite et un pire cas qui reste correct, juste plus lent.
    pub async fn wait_for_initialize_outcome(&self) -> Option<Result<Value, Value>> {
        if let Some(v) = self.cached_initialize_outcome() {
            return Some(v);
        }
        let notified = self.inner.initialize_notify.notified();
        tokio::select! {
            () = notified => {}
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
        self.cached_initialize_outcome()
    }

    fn cached_initialize_outcome(&self) -> Option<Result<Value, Value>> {
        self.inner
            .initialize_outcome
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Test-and-set : rend `true` si CET appelant est le premier à demander l'ouverture
    /// de `uri` (doit alors envoyer `didOpen`), `false` si `uri` est déjà ouverte par
    /// un autre client (éditeur navigateur ou un appel de tool MCP précédent — il ne
    /// faut RIEN envoyer, un second `didOpen` sur la même URI est une violation de
    /// protocole LSP). Ne suit pas les fermetures (`didClose`) : une URI ouverte le
    /// reste jusqu'à la mort du process — cohérent avec l'absence actuelle de
    /// `didClose` côté `LspClient` (cf. son `Drop`, qui ne fait que `unsubscribe`).
    pub fn try_mark_uri_open(&self, uri: &str) -> bool {
        match self.inner.open_uris.lock() {
            Ok(mut open) => open.insert(uri.to_string()),
            Err(mut poisoned) => poisoned.get_mut().insert(uri.to_string()),
        }
    }

    /// Diagnostics en cache pour `uri`, tel que publiés en dernier par le process —
    /// `None` si jamais publiés pour cette URI.
    pub fn cached_diagnostics(&self, uri: &str) -> Option<Vec<Value>> {
        self.inner
            .diagnostics_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(uri).cloned())
    }

    /// Attend que `uri` ait des diagnostics en cache — retour immédiat si déjà
    /// présents (même après un `didOpen` envoyé par un AUTRE client il y a longtemps,
    /// cf. doc du champ `diagnostics_cache`), sinon attend `diagnostics_notify`, borné
    /// à `timeout`.
    ///
    /// Rend `Some(vec)` dès qu'un `publishDiagnostics` a été vu pour `uri` — `vec`
    /// vide inclus, ça VEUT DIRE "le serveur a analysé et n'a rien trouvé", pas
    /// "on n'a rien reçu". `None` seulement si rien n'a jamais été publié dans le
    /// délai — état distinct, pas silencieusement confondu avec "propre" (trouvé
    /// dans un retour d'usage réel : un agent ne peut pas distinguer les deux avec
    /// un simple vecteur vide comme seul signal).
    ///
    /// Même course bénigne que `wait_for_initialize_outcome` (`notify_waiters` sans
    /// permit) — sans conséquence sur l'exactitude, seulement sur la latence dans une
    /// fenêtre étroite.
    pub async fn wait_for_diagnostics(
        &self,
        uri: &str,
        timeout: std::time::Duration,
    ) -> Option<Vec<Value>> {
        if let Some(d) = self.cached_diagnostics(uri) {
            return Some(d);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.inner.diagnostics_notify.notified();
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return self.cached_diagnostics(uri);
            }
            tokio::select! {
                () = notified => {}
                () = tokio::time::sleep(remaining) => {}
            }
            if let Some(d) = self.cached_diagnostics(uri) {
                return Some(d);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
        }
    }

    /// Version suivante pour `uri` (démarre à 2, +1 par appel). Compteurs des
    /// tools uniquement (cas A) — la version du `didOpen` navigateur (côté
    /// codemirror) est indépendante et ne passe jamais par ici (cf. doc du champ
    /// `doc_versions` : deux émetteurs actifs sur la même URI = désync interdit).
    ///
    /// Démarre à 2 et non à 1 : le `didOpen` émis par `ensure_open` porte la
    /// version 1, un `didChange` doit être strictement au-dessus pour que la
    /// séquence vue par le serveur reste monotone (1 → 2 → 3 …).
    pub fn next_doc_version(&self, uri: &str) -> i32 {
        let mut versions = self
            .inner
            .doc_versions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let next = versions.get(uri).copied().unwrap_or(1) + 1;
        versions.insert(uri.to_string(), next);
        next
    }

    /// Retire l'entrée `uri` du `diagnostics_cache`. Indispensable avant une
    /// ré-analyse : `wait_for_diagnostics` retourne le cache s'il est présent —
    /// sans invalidation, edit_and_check verrait le stale d'AVANT l'édition
    /// (design §7 étape 2). Aucun notify, aucun effet si absent.
    pub fn invalidate_diagnostics(&self, uri: &str) {
        self.inner
            .diagnostics_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(uri);
    }

    /// Un éditeur navigateur tient-il cette URI (cas B, design R1) ?
    pub fn has_editor_client(&self, uri: &str) -> bool {
        let uris = self
            .inner
            .editor_uris
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        uris.get(uri).is_some_and(|clients| !clients.is_empty())
    }

    /// Mark par le bridge `ws/lsp.rs` (`textDocument/didOpen`) : l'éditeur
    /// `client` tient `uri`. `unsubscribe(client)` DOIT retirer `client` de
    /// toutes les sets de `editor_uris` (nettoyage déconnexion).
    pub fn mark_editor_uri_open(&self, uri: &str, client: ClientId) {
        let mut uris = self
            .inner
            .editor_uris
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        uris.entry(uri.to_string()).or_default().insert(client);
    }

    /// Close par le bridge `ws/lsp.rs` (`textDocument/didClose`) : l'éditeur
    /// `client` lâche `uri`. L'URI cesse d'être tenue seulement si plus aucun
    /// client ne la tient (les autres tenants ne sont affectés en rien).
    pub fn mark_editor_uri_close(&self, uri: &str, client: ClientId) {
        let mut uris = self
            .inner
            .editor_uris
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(clients) = uris.get_mut(uri) {
            clients.remove(&client);
            if clients.is_empty() {
                uris.remove(uri);
            }
        }
    }
}

impl Drop for LspSessionInner {
    fn drop(&mut self) {
        // Invariant 6 : Drop tue le primaire ET tous les aux. Flag alive baissé
        // d'abord : la lectrice d'un aux tué ici ne doit pas sonner « degraded »
        // — c'est la mort du multiplexeur lui-même, pas une dégradation.
        self.alive.store(false, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            drop(child.kill());
        }
        for aux in &self.aux {
            aux.alive.store(false, Ordering::SeqCst);
            if let Some(mut child) = aux.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                drop(child.kill());
            }
        }
    }
}

// ── Manager ──────────────────────────────────────────────────────────────────

/// Gestionnaire des sessions LSP d'une sandbox : une session par toolchain.
pub struct LspManager {
    specs: HashMap<String, LspToolchain>,
    sandbox_root: PathBuf,
    sessions: Mutex<HashMap<String, Arc<LspSession>>>,
}

impl Clone for LspManager {
    fn clone(&self) -> Self {
        LspManager {
            specs: self.specs.clone(),
            sandbox_root: self.sandbox_root.clone(),
            sessions: Mutex::new(
                self.sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
        }
    }
}

impl Default for LspManager {
    /// Aucune toolchain LSP, sandbox_root `/workspace`.
    fn default() -> Self {
        LspManager::new(vec![], PathBuf::from("/workspace"))
    }
}

impl LspManager {
    pub fn new(specs: Vec<LspToolchain>, sandbox_root: PathBuf) -> Self {
        let mut map = HashMap::new();
        for spec in specs {
            map.insert(spec.name.clone(), spec);
        }
        LspManager {
            specs: map,
            sandbox_root,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Construit depuis `lsp_toolchains_from_env()` et `sandbox_root`.
    pub fn from_env(sandbox_root: PathBuf) -> anyhow::Result<Self> {
        let specs = lsp_toolchains_from_env()?;
        Ok(LspManager::new(specs, sandbox_root))
    }

    /// `true` si une toolchain LSP de ce nom est configurée.
    pub fn has(&self, toolchain: &str) -> bool {
        self.specs.contains_key(toolchain)
    }

    /// Rend la session existante si vivante, sinon spawn une neuve.
    /// `Ok(None)` si aucune toolchain de ce nom n'est configurée.
    pub async fn get_or_spawn(&self, toolchain: &str) -> anyhow::Result<Option<Arc<LspSession>>> {
        // Vérifier si la toolchain est configurée
        let spec = if let Some(spec) = self.specs.get(toolchain) {
            spec.clone()
        } else {
            return Ok(None);
        };

        // Vérifier s'il existe une session vivante
        {
            if let Some(session) = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(toolchain)
                && session.is_alive()
            {
                return Ok(Some(Arc::clone(session)));
            }
        }

        // Session morte ou inexistante : spawn une nouvelle
        let new_session = LspSession::spawn(&spec, &self.sandbox_root).await?;

        // Réinsérer (remplace une éventuelle ancienne entrée morte).
        // Si une autre tâche a déjà remplacé par sa session, accepter la sienne.
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = sessions.get(toolchain)
                && existing.is_alive()
                && !Arc::ptr_eq(existing, &new_session)
            {
                // Une autre tâche a spawné — l'utiliser à la place
                return Ok(Some(Arc::clone(existing)));
            }
            // Notre session est la plus récente, l'insérer
            sessions.insert(toolchain.to_string(), Arc::clone(&new_session));
        }

        Ok(Some(new_session))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::time::Duration;

    /// Script Python factice : echo — lit des trames stdin et répond.
    const FAKE_LSP_PY: &str = r#"
import sys

def read_message():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    lines = text.strip().split("\r\n")
    length = 0
    for line in lines:
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    payload = b""
    while len(payload) < length:
        chunk = sys.stdin.buffer.read(length - len(payload))
        if not chunk:
            break
        payload += chunk
    return payload

while True:
    msg = read_message()
    if not msg:
        break
    try:
        import json
        data = json.loads(msg)
        if "id" in data:
            reply = {"jsonrpc": "2.0", "id": data["id"], "result": f"pong:{data['id']}"}
        else:
            reply = {"jsonrpc": "2.0", "method": "fake/notify", "params": {}}
        out = json.dumps(reply).encode("utf-8")
        header = f"Content-Length: {len(out)}\r\n\r\n".encode("ascii")
        sys.stdout.buffer.write(header)
        sys.stdout.buffer.write(out)
        sys.stdout.buffer.flush()
    except Exception:
        pass
"#;

    /// Script Python factice : exit immédiat.
    const FAKE_LSP_EXIT_PY: &str = "import sys; sys.exit(0)\n";

    /// Script Python factice : publie des diagnostics à CHAQUE `didOpen`
    /// (contrairement à `FAKE_LSP_PY`, une seule fois par URI). Nécessaire au test
    /// d'invalidation : il faut DEUX publications successives sur la MÊME URI pour
    /// observer la différence entre le cache invalide et le frais — un fake qui ne
    /// publie qu'une fois par URI rend l'invalidation indistinguishable d'un cache
    /// qui n'a jamais rien contenu après le premier push.
    const FAKE_LSP_REPUBLISH_PY: &str = r#"
import sys, json

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

pub_count = 0
while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
        method = msg.get("method", "")
        if method == "textDocument/didOpen":
            uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
            notif = {
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": uri,
                    "diagnostics": [{"message": f"diag #{pub_count}", "severity": 1, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}]
                }
            }
            pub_count += 1
            write_frame(notif)
        elif "id" in msg:
            write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}})
    except Exception:
        pass
"#;

    /// Script Python factice : garde la DERNIÈRE notification reçue (global
    /// Python + octets bruts de la trame reconstruite header+payload) et la
    /// restitue comme `result` d'une requête `x/echo-notif`. Les notifications
    /// JSON-RPC n'ont pas de réponse, donc pas de corrélation possible : c'est
    /// le mécanisme imposé par la tâche pour rendre `didChange` observable,
    /// framing inclus (`_frameTerminators` = comptes de `\r\n\r\n` dans la
    /// trame reconstruite, posé par `encode_message` de la tâche écrivaine).
    const FAKE_LSP_ECHO_NOTIF_PY: &str = r#"
import sys, json

last_notification = None
last_frame = b""

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None, None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b"", header
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data, header + data

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

while True:
    payload, frame = read_frame()
    if payload is None:
        break
    try:
        msg = json.loads(payload)
    except Exception:
        continue
    if "id" not in msg:
        # Notification : conservée (parse + trame brute) pour x/echo-notif.
        last_notification = msg
        last_frame = frame or b""
        continue
    method = msg.get("method", "")
    if method == "x/echo-notif":
        result = dict(last_notification) if last_notification is not None else {"last": None}
        result["_frame"] = last_frame.decode("utf-8", errors="replace")
        result["_frameTerminators"] = last_frame.count(b"\r\n\r\n")
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": result})
    else:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}})
"#;

    /// Script Python factice enregistreur (multiplexeur, tâche 03) : lit stdin
    /// trame par trame, journalise chaque objet reçu (JSON un par ligne) dans le
    /// fichier passé en argv[1], et répond `{"result":"ok"}` aux requêtes
    /// (`"id"` présent). Le fichier est créé à la PREMIÈRE trame (mode append à
    /// chaque écriture) : « fichier absent » = « recorder jamais alimenté » —
    /// seul état observable d'un enfant qu'on n'a volontairement pas spawné.
    const FAKE_LSP_RECORDER_PY: &str = r#"
import sys, json

LOG = sys.argv[1] if len(sys.argv) > 1 else ""

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(msg) + "\n")
    if "id" in msg:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": "ok"})
"#;

    /// Chemin du log du recorder primaire posé par `make_fake_composite`.
    fn primary_log_path(tmpdir: &tempfile::TempDir) -> PathBuf {
        tmpdir.path().join("primary.log")
    }

    /// Chemin du log du recorder aux `index` posé par `make_fake_composite`.
    fn aux_log_path(tmpdir: &tempfile::TempDir, index: usize) -> PathBuf {
        tmpdir.path().join(format!("aux_{index}.log"))
    }

    /// Construit un `init_options` (`Value::Null` = rien à injecter) à partir de
    /// paires `(clé, JSON inline)` ; une valeur non-JSON est stockée en chaîne.
    fn build_init_options(pairs: &[(&str, &str)]) -> Value {
        if pairs.is_empty() {
            return Value::Null;
        }
        let mut map = serde_json::Map::new();
        for (key, raw) in pairs {
            let value =
                serde_json::from_str(raw).unwrap_or_else(|_| Value::String((*raw).to_string()));
            map.insert((*key).to_string(), value);
        }
        Value::Object(map)
    }

    /// Toolchain composite : script primaire + scripts aux (recorders), chacun
    /// avec son fichier-log dédié dans le tmpdir (nommé déterministement —
    /// `primary.log`, `aux_{i}.log` — les tests recalculent les chemins via
    /// `primary_log_path`/`aux_log_path`). Les logs ne sont PAS pré-créés :
    /// absence = recorder jamais alimenté. Chaque aux reçoit son chemin de log
    /// en argv[1] ; le primaire aussi (inoffensif pour les fakes non-recorders,
    /// qui ignorent l'argument de plus). Le tmpdir est retenu pour la durée du
    /// test (scripts + logs vivent dedans).
    /// Le tuple `(&role, &script, Vec<(clé, JSON)> )` des specs aux est celui
    /// imposé par la tâche 03 — d'où l'allow (signature documentée du helper).
    #[allow(clippy::type_complexity)]
    async fn make_fake_composite(
        primary_script: &str,
        aux_scripts: &[(
            &str,              /*role*/
            &str,              /*script*/
            Vec<(&str, &str)>, /*initOptions*/
        )],
    ) -> (LspToolchain, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let primary_script_path = tmpdir.path().join("fake_lsp_primary.py");
        std::fs::write(&primary_script_path, primary_script).unwrap();
        let mut aux = Vec::new();
        for (i, (role, script, init_options)) in aux_scripts.iter().enumerate() {
            let script_path = tmpdir.path().join(format!("fake_lsp_aux_{i}.py"));
            std::fs::write(&script_path, script).unwrap();
            let log_path = tmpdir.path().join(format!("aux_{i}.log"));
            aux.push(LspAux {
                role: role.to_string(),
                bin: "python3".to_string(),
                args: vec![
                    script_path.to_string_lossy().to_string(),
                    log_path.to_string_lossy().to_string(),
                ],
                init_options: build_init_options(init_options),
            });
        }
        let toolchain = LspToolchain {
            name: "composite".to_string(),
            bin: "python3".to_string(),
            args: vec![
                primary_script_path.to_string_lossy().to_string(),
                primary_log_path(&tmpdir).to_string_lossy().to_string(),
            ],
            aux,
        };
        (toolchain, tmpdir)
    }

    /// Lit un log de recorder (JSON un par ligne). Fichier absent = 0 trame.
    fn read_recorder_frames(path: &Path) -> Vec<Value> {
        match std::fs::read_to_string(path) {
            Ok(content) => content
                .lines()
                .filter(|line| !line.is_empty())
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Relit un log de recorder (poll 10ms, borné par `timeout`) jusqu'à ce que
    /// `done` soit satisfait des trames vues ; rend les trames lues (au plus
    /// une dernière relecture après le délai).
    async fn poll_recorder(
        path: &Path,
        done: impl Fn(&[Value]) -> bool,
        timeout: Duration,
    ) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let frames = read_recorder_frames(path);
            if done(&frames) || tokio::time::Instant::now() >= deadline {
                return frames;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Crée une toolchain LSP pointant vers un script Python factice.
    async fn make_fake_toolchain(name: &str, script: &str) -> (LspToolchain, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join(format!("fake_lsp_{name}.py"));
        std::fs::write(&script_path, script).unwrap();
        let toolchain = LspToolchain {
            name: name.to_string(),
            bin: "python3".to_string(),
            args: vec![script_path.to_string_lossy().to_string()],
            aux: vec![],
        };
        (toolchain, tmpdir)
    }

    /// Timeout de réception sur le channel d'un client.
    async fn recv_timeout<T>(rx: &mut mpsc::UnboundedReceiver<T>, timeout: Duration) -> Option<T> {
        tokio::time::timeout(timeout, rx.recv())
            .await
            .ok()
            .flatten()
    }

    // ── Tests unitaires (encode, FrameReader, parse) ───────────────────────

    /// Test 1: encode_has_content_length_header
    #[test]
    fn encode_has_content_length_header() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let encoded = encode_message(payload);
        let prefix = format!("Content-Length: {}\r\n\r\n", payload.len());
        assert!(
            encoded.starts_with(prefix.as_bytes()),
            "expected Content-Length header"
        );
        assert_eq!(
            &encoded[prefix.len()..],
            payload,
            "payload must end with the raw payload"
        );
    }

    /// Test 2: frame_reader_reassembles_split_chunks
    #[test]
    fn frame_reader_reassembles_split_chunks() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let encoded = encode_message(payload);

        let mut reader = FrameReader::new();
        // Découper en chunks de 3 octets
        let chunks: Vec<Vec<u8>> = encoded.chunks(3).map(|c| c.to_vec()).collect();
        // Pousser un par un
        for chunk in &chunks {
            reader.push(chunk);
        }

        // next() doit rendre le payload
        let result = reader.next_frame().expect("should yield the full payload");
        assert_eq!(result, payload, "decoded payload must match original");

        // next_frame() supplémentaire → None
        assert!(
            reader.next_frame().is_none(),
            "should return None after full frame decoded"
        );
    }

    /// Test 3: frame_reader_reads_two_messages
    #[test]
    fn frame_reader_reads_two_messages() {
        let p1 = encode_message(b"hello");
        let p2 = encode_message(b"world");
        let mut reader = FrameReader::new();
        reader.push(&p1);
        reader.push(&p2);

        assert_eq!(reader.next_frame().expect("msg1"), b"hello");
        assert_eq!(reader.next_frame().expect("msg2"), b"world");
        assert!(
            reader.next_frame().is_none(),
            "should return None after both frames"
        );
    }

    /// Test 4: parse_lsp_toolchains_valid_json
    #[test]
    fn parse_lsp_toolchains_valid_json() {
        let json = r#"[{"name":"rust","bin":"/toolchains/rust/bin/rust-analyzer","args":[]}]"#;
        let specs = parse_lsp_toolchains(json).expect("should parse valid JSON");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "rust");
        assert_eq!(specs[0].bin, "/toolchains/rust/bin/rust-analyzer");
        assert!(specs[0].args.is_empty());
    }

    /// Test 5: parse_lsp_toolchains_empty_array
    #[test]
    fn parse_lsp_toolchains_empty_array() {
        let specs = parse_lsp_toolchains("[]").expect("empty array should be valid");
        assert!(specs.is_empty(), "empty array should yield zero specs");
    }

    /// Test 6: parse_lsp_toolchains_invalid_json_errors
    #[test]
    fn parse_lsp_toolchains_invalid_json_errors() {
        let result = parse_lsp_toolchains("not json");
        assert!(result.is_err(), "invalid JSON should return Err");
    }

    // ── Tests manager ──────────────────────────────────────────────────────

    /// Test 7: manager_unknown_toolchain_returns_none
    #[tokio::test]
    async fn manager_unknown_toolchain_returns_none() {
        let root = PathBuf::from("/tmp/test-root");
        let manager = LspManager::new(vec![], root);
        let result = manager
            .get_or_spawn("nope")
            .await
            .expect("get_or_spawn should succeed");
        assert!(result.is_none(), "unknown toolchain should return None");
    }

    /// Test 8: manager_reuses_alive_session
    #[tokio::test]
    async fn manager_reuses_alive_session() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let manager = LspManager::new(vec![spec], root);

        let s1 = manager
            .get_or_spawn("fake")
            .await
            .expect("first spawn should succeed");
        assert!(s1.is_some(), "should return Some");
        let s1 = s1.unwrap();
        assert!(s1.is_alive(), "session should be alive");

        // Deuxième appel → same session (réutilisée)
        let s2 = manager
            .get_or_spawn("fake")
            .await
            .expect("reuse should succeed");
        assert!(s2.is_some(), "should return Some on reuse");
        assert!(
            Arc::ptr_eq(&s1, &s2.unwrap()),
            "should return the same session (same Arc)"
        );
        drop(tmpdir);
    }

    // ── Tests cache initialize (bug réel : double initialize, cf. ws/lsp.rs) ─────

    /// Un second appelant (`try_mark_initialized` déjà `false`) ne doit jamais
    /// renvoyer `initialize` au process — mais doit récupérer l'issue du premier via
    /// le cache, immédiatement si déjà posée (cas succès).
    #[tokio::test]
    async fn initialize_outcome_cached_and_replayed_immediately() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn should succeed");

        assert!(session.try_mark_initialized(), "first caller should win");
        assert!(!session.try_mark_initialized(), "second caller should lose");

        let capabilities = serde_json::json!({"hoverProvider": true});
        session.set_initialize_outcome(Ok(capabilities.clone()));

        let cached = session
            .wait_for_initialize_outcome()
            .await
            .expect("outcome should be cached");
        assert_eq!(cached, Ok(capabilities));
    }

    /// Un premier `initialize` en ÉCHEC (ex. typescript-language-server sans
    /// `node_modules` local) doit aussi être mis en cache et rejoué tel quel — sinon
    /// tout client suivant attend le timeout de 30s pour rien (bug trouvé en usage
    /// réel).
    #[tokio::test]
    async fn initialize_outcome_caches_failure_too() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn should succeed");

        assert!(session.try_mark_initialized());
        assert!(!session.try_mark_initialized());

        let error = serde_json::json!({"code": -32603, "message": "Could not find a valid TypeScript installation"});
        session.set_initialize_outcome(Err(error.clone()));

        let cached = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            session.wait_for_initialize_outcome(),
        )
        .await
        .expect("failure must be cached immediately, not wait for the 30s timeout")
        .expect("outcome should be cached");
        assert_eq!(cached, Err(error));
    }

    /// Un appelant qui attend AVANT que le premier ait fini son `initialize` réel
    /// doit être réveillé par `notify_waiters` dès que l'issue est posée, pas bloqué
    /// jusqu'au timeout de 30s.
    #[tokio::test]
    async fn initialize_outcome_wakes_pending_waiter() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn should succeed");

        assert!(session.try_mark_initialized());
        assert!(!session.try_mark_initialized());

        let waiter_session = Arc::clone(&session);
        let waiter =
            tokio::spawn(async move { waiter_session.wait_for_initialize_outcome().await });

        // Laisser le waiter s'enregistrer avant de poser le résultat.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let capabilities = serde_json::json!({"definitionProvider": true});
        session.set_initialize_outcome(Ok(capabilities.clone()));

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("waiter should be woken well before the 5s test timeout")
            .expect("waiter task should not panic");
        assert_eq!(result, Some(Ok(capabilities)));
    }

    // ── Tests session ──────────────────────────────────────────────────────

    /// Test 9: session_request_response
    #[tokio::test]
    async fn session_request_response() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_id, mut rx) = session.subscribe();

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping",
            "params": {}
        });
        let payload = request.to_string().into_bytes();
        session
            .send(client_id, payload)
            .await
            .expect("send should succeed");

        // Attendre la réponse (timeout 5s)
        let resp = recv_timeout(&mut rx, Duration::from_secs(5)).await;
        assert!(resp.is_some(), "should receive response within timeout");

        let raw = resp.unwrap();
        let resp = serde_json::from_slice::<Value>(&raw).expect("response should be valid JSON");
        assert_eq!(
            resp["id"].as_i64().unwrap(),
            1,
            "id should be restored to original"
        );
        // resultat = pong:<session_id> (l'id réécrit par la session que le fake LSP echo)
        assert!(
            resp["result"].as_str().unwrap().starts_with("pong:"),
            "result must be pong:<session_id>"
        );

        drop(tmpdir);
    }

    /// Test 10: session_routes_same_id_to_each_client
    #[tokio::test]
    async fn session_routes_same_id_to_each_client() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_a, mut rx_a) = session.subscribe();
        let (client_b, mut rx_b) = session.subscribe();

        // A envoie id:1
        let req_a = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}})
            .to_string()
            .into_bytes();
        session.send(client_a, req_a).await.expect("A send ok");

        // B envoie id:1
        let req_b = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}})
            .to_string()
            .into_bytes();
        session.send(client_b, req_b).await.expect("B send ok");

        // Chaque client reçoit sa réponse avec id == 1
        let resp_a = recv_timeout(&mut rx_a, Duration::from_secs(5)).await;
        let resp_b = recv_timeout(&mut rx_b, Duration::from_secs(5)).await;

        assert!(resp_a.is_some(), "A should receive response");
        assert!(resp_b.is_some(), "B should receive response");

        let r_a = serde_json::from_slice::<Value>(&resp_a.unwrap()).unwrap();
        let r_b = serde_json::from_slice::<Value>(&resp_b.unwrap()).unwrap();

        assert_eq!(
            r_a["id"].as_i64().unwrap(),
            1,
            "A's response id should be 1"
        );
        assert_eq!(
            r_b["id"].as_i64().unwrap(),
            1,
            "B's response id should be 1"
        );

        // result DIFFÈRE → ids de session différents → pas de cross-routing
        let result_a = r_a["result"].as_str().unwrap();
        let result_b = r_b["result"].as_str().unwrap();
        assert_ne!(
            result_a, result_b,
            "results must differ for different clients"
        );

        drop(tmpdir);
    }

    /// Test 11: session_broadcasts_notifications
    #[tokio::test]
    async fn session_broadcasts_notifications() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_a, mut rx_a) = session.subscribe();
        let (_, mut rx_b) = session.subscribe();

        // A envoie une notification (pas d'id)
        let notif = serde_json::json!(
            {"jsonrpc":"2.0","method":"fake/notify","params":{}}
        )
        .to_string()
        .into_bytes();
        session
            .send(client_a, notif)
            .await
            .expect("send notification ok");

        // B ET A reçoivent la notification
        let notif_b = recv_timeout(&mut rx_b, Duration::from_secs(5))
            .await
            .expect("B must receive notification");
        let notif_a = recv_timeout(&mut rx_a, Duration::from_secs(5))
            .await
            .expect("A must also receive its own notification");

        let n_b = serde_json::from_slice::<Value>(&notif_b).unwrap();
        let n_a = serde_json::from_slice::<Value>(&notif_a).unwrap();

        assert_eq!(
            n_b["method"].as_str().unwrap(),
            "fake/notify",
            "B: method should match"
        );
        assert_eq!(
            n_a["method"].as_str().unwrap(),
            "fake/notify",
            "A: method should match"
        );

        drop(tmpdir);
    }

    /// Test 12: send_rejects_invalid_json
    #[tokio::test]
    async fn send_rejects_invalid_json() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _) = session.subscribe();

        let result = session.send(client, b"not json".to_vec()).await;
        assert!(result.is_err(), "invalid JSON should return Err");
        assert!(
            result.unwrap_err().to_string().contains("VNL-SBX-LSP-001"),
            "error message must contain VNL-SBX-LSP-001"
        );

        drop(tmpdir);
    }

    /// Test 13: send_rejects_non_integer_id
    #[tokio::test]
    async fn send_rejects_non_integer_id() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _) = session.subscribe();

        let payload = serde_json::json!({"jsonrpc":"2.0","id":"str","method":"ping","params":{}})
            .to_string()
            .into_bytes();
        let result = session.send(client, payload).await;
        assert!(result.is_err(), "non-integer id should return Err");
        assert!(
            result.unwrap_err().to_string().contains("VNL-SBX-LSP-002"),
            "error must contain VNL-SBX-LSP-002"
        );

        drop(tmpdir);
    }

    /// Test 13b: send_rejects_float_id — un id flottant est non-entier → VNL-SBX-LSP-002.
    #[tokio::test]
    async fn send_rejects_float_id() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _) = session.subscribe();

        let payload = serde_json::json!({"jsonrpc":"2.0","id":1.5,"method":"ping","params":{}})
            .to_string()
            .into_bytes();
        let result = session.send(client, payload).await;
        assert!(result.is_err(), "float id should return Err");
        assert!(
            result.unwrap_err().to_string().contains("VNL-SBX-LSP-002"),
            "error must contain VNL-SBX-LSP-002"
        );

        drop(tmpdir);
    }

    /// Test 14: manager_respawns_dead_session
    #[tokio::test]
    async fn manager_respawns_dead_session() {
        let (spec, tmpdir) = make_fake_toolchain("die", FAKE_LSP_EXIT_PY).await;
        let root = tmpdir.path().to_path_buf();
        let manager = LspManager::new(vec![spec], root);

        // Spawn la session morte
        let s1 = manager
            .get_or_spawn("die")
            .await
            .expect("first spawn ok")
            .unwrap();

        // Attendre que le process meure (poll 10ms, timeout 5s)
        for _ in 0..500 {
            if !s1.is_alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!s1.is_alive(), "process should be dead by now");

        // get_or_spawn doit respawn
        let s2 = manager
            .get_or_spawn("die")
            .await
            .expect("respawn ok")
            .unwrap();
        assert!(
            !Arc::ptr_eq(&s1, &s2),
            "should return a new session (different Arc)"
        );

        drop(tmpdir);
    }

    // ── Tests tâche 08a : versions de doc, invalidation, didChange, suivi éditeur ──

    /// Test 15: next_doc_version_increments_per_uri — un compteur par URI, démarre
    /// à 2 (le `didOpen` a la version 1), +1 par appel ; les URI n'interagissent
    /// pas (les compteurs navigateur sont de toute façon indépendants —
    /// `next_doc_version` ne sert qu'aux tools).
    #[tokio::test]
    async fn next_doc_version_increments_per_uri() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        assert_eq!(session.next_doc_version("file:///a.rs"), 2);
        assert_eq!(session.next_doc_version("file:///a.rs"), 3);
        assert_eq!(session.next_doc_version("file:///a.rs"), 4);
        // URI indépendante : jamais vue → redémarre à 2.
        assert_eq!(session.next_doc_version("file:///b.rs"), 2);

        drop(tmpdir);
    }

    /// Test 16: invalidate_diagnostics_then_wait_gets_fresh — le test qui prouve
    /// l'utilité de l'invalidation (design §7 étape 2) : `wait_for_diagnostics`
    /// retourne le cache s'il est présent ; sans `invalidate_diagnostics` avant
    /// l'édition, `edit_and_check` reverrait le stale d'AVANT l'édition.
    #[tokio::test]
    async fn invalidate_diagnostics_then_wait_gets_fresh() {
        let (spec, tmpdir) = make_fake_toolchain("republish", FAKE_LSP_REPUBLISH_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _rx) = session.subscribe();
        let uri = "file:///workspace/main.rs";

        let open_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn main(){}"
                }
            }
        })
        .to_string()
        .into_bytes();

        // Premier didOpen → publication "diag #0" mise en cache.
        session
            .send(client, open_payload.clone())
            .await
            .expect("didOpen #1 ok");
        let first = session
            .wait_for_diagnostics(uri, Duration::from_secs(5))
            .await
            .expect("first publish should be cached");
        assert_eq!(first[0]["message"].as_str().unwrap(), "diag #0");
        assert!(session.cached_diagnostics(uri).is_some());

        // Invalidation → le cache est vide, sans notify ni effet de bord.
        session.invalidate_diagnostics(uri);
        assert!(
            session.cached_diagnostics(uri).is_none(),
            "invalidate must remove the cache entry"
        );

        // Le wait ne doit PAS ressusciter le stale : bloqué jusqu'au timeout
        // court, il rend None tant que rien de nouveau n'est publié.
        let started = tokio::time::Instant::now();
        let stale = session
            .wait_for_diagnostics(uri, Duration::from_millis(250))
            .await;
        assert!(
            stale.is_none(),
            "wait must not return the invalidated (stale) cache"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "wait should genuinely block on an empty cache, elapsed: {:?}",
            started.elapsed()
        );

        // Second publish (même URI) → le wait rend le FRAIS, pas l'ancien.
        session
            .send(client, open_payload)
            .await
            .expect("didOpen #2 ok");
        let fresh = session
            .wait_for_diagnostics(uri, Duration::from_secs(5))
            .await
            .expect("second publish should reach wait_for_diagnostics");
        assert_eq!(fresh[0]["message"].as_str().unwrap(), "diag #1");

        drop(tmpdir);
    }

    /// Test 17: did_change_frame_full_sync_shape — la notification `didChange` du
    /// client a exactement la forme full sync (design §7) : un seul
    /// `contentChanges` = `{"text": …}` sans `range`, la version passée portée
    /// PAR `textDocument.version` (`VersionedTextDocumentIdentifier` de la spec
    /// LSP — jamais un champ frère `textDocumentVersion`, qu'un vrai serveur
    /// ignorerait), et le framing Content-Length posé par la tâche écrivaine
    /// (un seul `\r\n\r\n` dans la trame reconstruite par le fake).
    #[tokio::test]
    async fn did_change_frame_full_sync_shape() {
        let (spec, tmpdir) = make_fake_toolchain("echonotif", FAKE_LSP_ECHO_NOTIF_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let mut client =
            crate::lsp_client::LspClient::new(session, "file:///workspace".to_string());
        let uri = "file:///workspace/main.rs";
        client
            .did_change(uri, 7, "texte")
            .await
            .expect("did_change ok");

        // Notifications non corrélables (pas de réponse JSON-RPC) : le fake garde
        // la dernière reçue et la restitue sur `x/echo-notif`.
        let echoed = client
            .request("x/echo-notif", serde_json::json!({}))
            .await
            .expect("x/echo-notif ok");

        assert_eq!(
            echoed["method"].as_str().unwrap(),
            "textDocument/didChange",
            "echoed notification must be the didChange"
        );
        assert_eq!(
            echoed["params"]["textDocument"]["uri"].as_str().unwrap(),
            uri
        );
        assert_eq!(
            echoed["params"]["textDocument"]["version"]
                .as_i64()
                .unwrap(),
            7,
            "version must be the one passed in, inside textDocument (VersionedTextDocumentIdentifier)"
        );
        assert!(
            echoed["params"]["textDocumentVersion"].is_null(),
            "no sibling textDocumentVersion — a real server would ignore it"
        );
        let changes = echoed["params"]["contentChanges"]
            .as_array()
            .expect("contentChanges must be an array");
        assert_eq!(changes.len(), 1, "full sync: exactly one entry");
        // Égalité exacte sur l'objet : `{"text": "texte"}` et RIEN d'autre —
        // notamment pas de `range` (full sync, jamais de changement partiel).
        assert_eq!(changes[0], serde_json::json!({"text": "texte"}));

        // Framing posé par `notify` → encode_message de la tâche écrivaine :
        // la trame reconstruite est `Content-Length: N\r\n\r\n{payload}` →
        // exactement un terminateur `\r\n\r\n`, et l'en-tête devant.
        assert_eq!(
            echoed["_frameTerminators"].as_i64().unwrap(),
            1,
            "frame must carry exactly one \\r\\n\\r\\n terminator"
        );
        assert!(
            echoed["_frame"]
                .as_str()
                .expect("frame must be utf-8 lossy decodable")
                .starts_with("Content-Length: "),
            "frame must start with the Content-Length header"
        );

        drop(tmpdir);
    }

    /// Test 18: editor_uri_tracking_open_close_unsubscribe — plusieurs tenants sur
    /// une même URI ; la déconnexion (unsubscribe) ou la fermeture (mark_close) de
    /// l'un ne libère pas l'URI tant qu'un autre la tient.
    #[tokio::test]
    async fn editor_uri_tracking_open_close_unsubscribe() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (c1, _rx1) = session.subscribe();
        let (c2, _rx2) = session.subscribe();
        let uri = "file:///workspace/a.rs";

        assert!(
            !session.has_editor_client(uri),
            "no editor client before any mark_open"
        );

        session.mark_editor_uri_open(uri, c1);
        assert!(session.has_editor_client(uri), "c1 holds the uri");
        session.mark_editor_uri_open(uri, c2);
        assert!(session.has_editor_client(uri), "two holders");

        // Déconnexion de c1 : c2 tient toujours l'URI.
        session.unsubscribe(c1);
        assert!(
            session.has_editor_client(uri),
            "uri still held by c2 after c1 unsubscribes"
        );
        session.unsubscribe(c2);
        assert!(
            !session.has_editor_client(uri),
            "no holder left after c2 unsubscribes"
        );

        // mark_close séparé sur c3 n'affecte pas c4.
        let (c3, _rx3) = session.subscribe();
        let (c4, _rx4) = session.subscribe();
        let uri2 = "file:///workspace/b.rs";
        session.mark_editor_uri_open(uri2, c3);
        session.mark_editor_uri_open(uri2, c4);
        session.mark_editor_uri_close(uri2, c3);
        assert!(
            session.has_editor_client(uri2),
            "c4 unaffected by c3's mark_close"
        );
        session.mark_editor_uri_close(uri2, c4);
        assert!(
            !session.has_editor_client(uri2),
            "last holder closed → false"
        );

        drop(tmpdir);
    }

    /// Test 19: unsubscribe_cleans_all_editor_uris — un client qui tient 3 URI
    /// (sans autres tenants) : la déconnexion libère les 3, nulle part d'autre ne
    /// reste marqué tenu (nettoyage déconnexion, piste R1 sq1).
    #[tokio::test]
    async fn unsubscribe_cleans_all_editor_uris() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (c1, _rx1) = session.subscribe();
        let uris = [
            "file:///workspace/x1.rs",
            "file:///workspace/x2.rs",
            "file:///workspace/x3.rs",
        ];
        for uri in uris {
            session.mark_editor_uri_open(uri, c1);
            assert!(session.has_editor_client(uri), "{uri} held by c1");
        }

        session.unsubscribe(c1);
        for uri in uris {
            assert!(
                !session.has_editor_client(uri),
                "{uri} must be released when its only holder unsubscribes"
            );
        }

        drop(tmpdir);
    }

    // ── Tests tâche 03 (vue-lsp) : ossature multi-process ──────────────────

    /// Test 20: parse_aux_roundtrip_and_default_empty — contrat de format
    /// `VNL_LSP_TOOLCHAINS` avec `aux` (interop tâche controller ultérieure) :
    /// valeurs exactes (dont `initOptions` → `init_options`), clé `aux` absente
    /// ⟹ `vec![]`, sérialisation sans aux ⟹ `"aux":[]` (additif, accepté).
    #[test]
    fn parse_aux_roundtrip_and_default_empty() {
        let json = r#"[
            {"name":"node","bin":"/toolchains/node-lsp/bin/vue-language-server","args":["--stdio"],
             "aux":[{"role":"tsserver-forward",
                     "bin":"/toolchains/node-lsp/bin/typescript-language-server","args":["--stdio"],
                     "initOptions":{"plugins":[{"name":"@vue/typescript-plugin",
                                                "location":"/toolchains/node-lsp/lib/node_modules/@vue/language-server"}]}}]}
        ]"#;
        let specs = parse_lsp_toolchains(json).expect("JSON avec aux doit parser");
        assert_eq!(specs[0].aux.len(), 1, "un seul aux");
        let a = &specs[0].aux[0];
        assert_eq!(a.role, "tsserver-forward");
        assert_eq!(a.bin, "/toolchains/node-lsp/bin/typescript-language-server");
        assert_eq!(a.args, vec!["--stdio".to_string()]);
        assert_eq!(
            a.init_options["plugins"],
            serde_json::json!([
                {"name": "@vue/typescript-plugin",
                 "location": "/toolchains/node-lsp/lib/node_modules/@vue/language-server"}
            ]),
            "initOptions doit atterrir dans init_options, verbatim"
        );

        // Clé `aux` absente (cas de tous les pods déployés aujourd'hui) → vec![].
        let json_no_aux =
            r#"[{"name":"rust","bin":"/toolchains/rust/bin/rust-analyzer","args":[]}]"#;
        let plain = parse_lsp_toolchains(json_no_aux).expect("JSON sans aux doit parser");
        assert!(
            plain[0].aux.is_empty(),
            "clé aux absente doit default à vec![]"
        );

        // Sérialisation d'un LspToolchain sans aux → "aux":[] (additif accepté).
        let serialized = serde_json::to_value(&plain[0]).unwrap();
        assert_eq!(serialized["aux"], serde_json::json!([]));

        // Round-trip complet du composite (contrat d'interop controller).
        let back: LspToolchain =
            serde_json::from_value(serde_json::to_value(&specs[0]).unwrap()).unwrap();
        assert_eq!(back.name, specs[0].name);
        assert_eq!(back.bin, specs[0].bin);
        assert_eq!(back.args, specs[0].args);
        assert_eq!(back.aux.len(), 1);
        assert_eq!(back.aux[0].role, a.role);
        assert_eq!(back.aux[0].bin, a.bin);
        assert_eq!(back.aux[0].args, a.args);
        assert_eq!(back.aux[0].init_options, a.init_options);
        assert!(
            serde_json::to_value(&back.aux[0])
                .unwrap()
                .get("initOptions")
                .is_some(),
            "init_options doit se sérialiser sous la clé JSON initOptions"
        );
    }

    /// Test 21: composite_initialize_reaches_both_injected_options_on_aux —
    /// `initialize` part au primaire (chemin actuel, options du client seules)
    /// ET à chaque aux vivant en copie avec `initOptions` du spec injectés
    /// (clés spec gagnantes, options client conservées) ; la réponse de l'aux
    /// est interceptée par la session : le client ne voit EXACTEMENT que la
    /// réponse du primaire (invariant 3).
    #[tokio::test]
    async fn composite_initialize_reaches_both_injected_options_on_aux() {
        let plugins_json = r#"[{"name":"@vue/typescript-plugin","location":"/opt/plugins/vue"}]"#;
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_RECORDER_PY,
            &[(
                "tsserver-forward",
                FAKE_LSP_RECORDER_PY,
                vec![("plugins", plugins_json)],
            )],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"processId": null, "initializationOptions": {"existing": true}}
        });
        session
            .send(client, init.to_string().into_bytes())
            .await
            .expect("send initialize ok");

        // (c) Le flux client reçoit une réponse id 1 — celle du primaire.
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("la réponse initialize du primaire doit parvenir au client");
        let resp = serde_json::from_slice::<Value>(&resp).unwrap();
        assert_eq!(resp["id"].as_i64(), Some(1), "id restauré côté client");
        assert!(
            resp.get("result").is_some(),
            "la réponse routée est un résultat"
        );

        // (a) Recorder primaire : initialize reçu avec les options du client,
        // PAS les initOptions de l'aux (injection réservée à l'enfant).
        let is_init = |f: &Value| f.get("method").and_then(|m| m.as_str()) == Some("initialize");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_init),
            Duration::from_secs(5),
        )
        .await;
        let prim = primary_frames
            .iter()
            .find(|f| is_init(f))
            .expect("le primaire doit recevoir initialize");
        assert_eq!(
            prim["params"]["initializationOptions"]["existing"],
            serde_json::json!(true)
        );
        assert!(
            prim["params"]["initializationOptions"]
                .get("plugins")
                .is_none(),
            "le primaire ne doit PAS recevoir les initOptions de l'aux: {prim}"
        );

        // (b) Recorder aux : copie initialize avec plugins = tableau du spec
        // ET clé client conservée (fusion superficielle, clés spec gagnantes).
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_init),
            Duration::from_secs(5),
        )
        .await;
        let aux_init = aux_frames
            .iter()
            .find(|f| is_init(f))
            .expect("l'aux doit recevoir une copie de initialize");
        assert_eq!(
            aux_init["params"]["initializationOptions"]["plugins"],
            serde_json::from_str::<Value>(plugins_json).unwrap(),
            "les initOptions du spec doivent être injectés verbatim"
        );
        assert_eq!(
            aux_init["params"]["initializationOptions"]["existing"],
            serde_json::json!(true),
            "les options du client doivent être conservées"
        );

        // Rien d'autre ne doit jamais suivre : la réponse de l'aux (ids interne)
        // est avalée par la session, jamais routée au client.
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "une réponse d'aux a fuité vers le client: {leaked:?}"
        );

        drop(tmpdir);
    }

    /// Test 22: composite_didOpen_fanout_to_both — toute notification client
    /// (doc-sync inclus) part telle quelle à chaque enfant vivant (invariant 2).
    /// Nom du test imposé tel quel par la tâche 03 (`didOpen` = méthode LSP).
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn composite_didOpen_fanout_to_both() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_RECORDER_PY,
            &[("tsserver-forward", FAKE_LSP_RECORDER_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        // initialize d'abord (séquence LSP réelle ; la fan-out didOpen est ce
        // qu'observe ce test).
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("initialize ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("réponse initialize du primaire");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(1)
        );

        let uri = "file:///workspace/App.vue";
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {"uri": uri, "languageId": "vue", "version": 1, "text": "<template/>"}
            }
        });
        session
            .send(client, did_open.to_string().into_bytes())
            .await
            .expect("didOpen ok");

        let saw_open = |frames: &[Value]| {
            frames.iter().any(|f| {
                f.get("method").and_then(|m| m.as_str()) == Some("textDocument/didOpen")
                    && f["params"]["textDocument"]["uri"].as_str() == Some(uri)
            })
        };
        let primary_frames =
            poll_recorder(&primary_log_path(&tmpdir), saw_open, Duration::from_secs(5)).await;
        let aux_frames =
            poll_recorder(&aux_log_path(&tmpdir, 0), saw_open, Duration::from_secs(5)).await;
        assert!(
            saw_open(&primary_frames),
            "le primaire doit recevoir le didOpen; trames: {primary_frames:?}"
        );
        assert!(
            saw_open(&aux_frames),
            "l'aux doit recevoir le même didOpen; trames: {aux_frames:?}"
        );

        drop(tmpdir);
    }

    /// Test 23: composite_other_request_primary_only — une requête (id) qui
    /// n'est pas `initialize` reste intégralement sur le primaire : réponse au
    /// client, l'aux ne la voit jamais (invariant 4 — routage/fusion : tâches
    /// 04–06, NE RIEN fusionner ici).
    #[tokio::test]
    async fn composite_other_request_primary_only() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_RECORDER_PY,
            &[("tsserver-forward", FAKE_LSP_RECORDER_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        // initialize + didOpen AVANT le hover : prouvent que le canal de l'aux
        // est vivant (l'absence du hover ensuite est significative, pas
        // l'artefact d'un aux qui n'aurait jamais rien reçu).
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("initialize ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("réponse initialize");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(1)
        );

        let uri = "file:///workspace/App.vue";
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": uri, "languageId": "vue", "version": 1, "text": ""}}
        });
        session
            .send(client, did_open.to_string().into_bytes())
            .await
            .expect("didOpen ok");
        let saw_open = |frames: &[Value]| {
            frames
                .iter()
                .any(|f| f.get("method").and_then(|m| m.as_str()) == Some("textDocument/didOpen"))
        };
        let aux_frames =
            poll_recorder(&aux_log_path(&tmpdir, 0), saw_open, Duration::from_secs(5)).await;
        assert!(
            saw_open(&aux_frames),
            "l'aux doit avoir reçu le didOpen (canal vivant) avant l'assertion hover; trames: {aux_frames:?}"
        );

        let hover = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
            "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}}
        });
        session
            .send(client, hover.to_string().into_bytes())
            .await
            .expect("hover send ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit répondre du hover");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(2)
        );

        let saw_hover = |frames: &[Value]| {
            frames
                .iter()
                .any(|f| f.get("method").and_then(|m| m.as_str()) == Some("textDocument/hover"))
        };
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            saw_hover,
            Duration::from_secs(5),
        )
        .await;
        assert!(
            saw_hover(&primary_frames),
            "le primaire doit recevoir le hover; trames: {primary_frames:?}"
        );

        // Fenêtre laissée à l'aux pour le recevoir éventuellement : rien.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !saw_hover(&aux_frames),
            "l'aux ne doit PAS recevoir de requête non-initialize; trames: {aux_frames:?}"
        );

        drop(tmpdir);
    }

    /// Test 24: primary_death_kills_session — EOF stdout du PRIMAIRE = mort de
    /// la session (invariant 6), même avec un aux durable à côté ; pattern de
    /// poll borné de `manager_respawns_dead_session`.
    #[tokio::test]
    async fn primary_death_kills_session() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_EXIT_PY,
            &[("tsserver-forward", FAKE_LSP_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");

        for _ in 0..500 {
            if !session.is_alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !session.is_alive(),
            "la mort du primaire doit emporter la session"
        );

        drop(tmpdir);
    }

    /// Test 25: aux_death_session_survives_degraded — EOF stdout d'un AUX =
    /// dégradation, pas mort (invariant 6) : `is_alive()` (primaire) reste
    /// `true`, les requêtes primaire répondent, le fan-out ignore l'aux mort
    /// sans erreur.
    #[tokio::test]
    async fn aux_death_session_survives_degraded() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_PY,
            &[("tsserver-forward", FAKE_LSP_EXIT_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("la mort attendue de l'aux ne doit pas faire échouer le spawn");

        // Laisser l'aux mourir (exit immédiat + EOF stdout constaté).
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            session.is_alive(),
            "la mort d'un aux ne doit pas tuer la session"
        );

        let (client, mut rx) = session.subscribe();
        let req = serde_json::json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{}});
        session
            .send(client, req.to_string().into_bytes())
            .await
            .expect("requête primaire ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit toujours répondre après perte de l'aux");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(3)
        );

        // didOpen ultérieur : le fan-out ignore l'aux mort, nulle part d'erreur.
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///workspace/App.vue", "languageId": "vue", "version": 1, "text": ""}}
        });
        session
            .send(client, did_open.to_string().into_bytes())
            .await
            .expect("didOpen ne doit pas erreur avec un aux mort");
        assert!(session.is_alive());

        drop(tmpdir);
    }

    /// Test 26: aux_spawn_failure_degrades_not_fails — un aux au binaire absent
    /// dégrade la session (`warn`) mais ne la fait jamais échouer (invariant 1) :
    /// `spawn` rend `Ok`, session vivante, primaire fonctionnel, notifications
    /// fan-out sans erreur.
    #[tokio::test]
    async fn aux_spawn_failure_degrades_not_fails() {
        let (mut spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        spec.aux.push(LspAux {
            role: "tsserver-forward".to_string(),
            bin: "/nonexistent/vanilla-lsp".to_string(),
            args: vec![],
            init_options: Value::Null,
        });
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("un aux inexistant doit dégrader la session, pas la faire échouer");
        assert!(session.is_alive());

        let (client, mut rx) = session.subscribe();
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":4,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("requête primaire ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit répondre malgré l'aux absent");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(4)
        );

        // Notification : le fan-out vers l'aux jamais né doit rester sans erreur.
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","method":"fake/notify","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("notification ok malgré aux jamais spawné");

        drop(tmpdir);
    }

    /// Test 27: aux_unknown_role_ignored — rôle inconnu au spawn : `warn` +
    /// enfant ignoré (JAMAIS spawné — son recorder ne crée même pas son log),
    /// session vivante et fonctionnelle avec les aux connus (invariant 1).
    #[tokio::test]
    async fn aux_unknown_role_ignored() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_RECORDER_PY,
            &[("wat", FAKE_LSP_RECORDER_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("un rôle aux inconnu ne doit pas faire échouer le spawn");
        assert!(session.is_alive());

        // Notification de sonde : le primaire (recorder) la reçoit, prouvant que
        // le fan-out a eu lieu — un aux inconnu qui aurait été spawné à tort
        // aurait reçu la même trame et créé son log.
        let (client, mut rx) = session.subscribe();
        let probe = serde_json::json!({"jsonrpc":"2.0","method":"fake/notify","params":{}});
        session
            .send(client, probe.to_string().into_bytes())
            .await
            .expect("notification ok");
        let saw_probe = |frames: &[Value]| {
            frames
                .iter()
                .any(|f| f.get("method").and_then(|m| m.as_str()) == Some("fake/notify"))
        };
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            saw_probe,
            Duration::from_secs(5),
        )
        .await;
        assert!(
            saw_probe(&primary_frames),
            "le primaire doit recevoir la sonde; trames: {primary_frames:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !aux_log_path(&tmpdir, 0).exists(),
            "le recorder d'un aux à rôle inconnu n'a jamais dû être spawné"
        );

        // Session pleinement fonctionnelle avec les aux connus (ici : aucun).
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":5,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("requête primaire ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("la session doit fonctionner normalement");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(5)
        );

        drop(tmpdir);
    }

    // ── Tests tâche 05 (vue-lsp) : forwarding tsserver/request ──────────────

    /// Script Python factice PRIMAIRE « vue-ish » (tâche 05) : journalise les
    /// trames reçues dans argv[1] (pattern recorder de la tâche 03) ; à chaque
    /// `textDocument/didOpen`, émet la notification `tsserver/request` avec
    /// `params = [[id_vue, "_vue:projectInfo", {"file": <uri}>]]` (`id_vue`
    /// compte 1, 2, 3… — comme vue-language-server en mode hybride Volar v3) ;
    /// répond `{"result":{"echo":…}}` à toute requête. Mode spécial via
    /// `fake_mode` : `"garbage"` ⟹ n'émet plus que `params: "garbage"` puis un
    /// bon `[[3, "_vue:x", null]]` (test des trames malformées).
    const FAKE_LSP_VUEISH_PY: &str = r#"
import sys, json

LOG = sys.argv[1] if len(sys.argv) > 1 else ""
MODE = "@@MODE@@"

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

vue_id = 0
while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(msg) + "\n")
    method = msg.get("method", "")
    if method == "textDocument/didOpen":
        uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
        if MODE == "garbage":
            write_frame({"jsonrpc": "2.0", "method": "tsserver/request", "params": "garbage"})
            write_frame({"jsonrpc": "2.0", "method": "tsserver/request",
                         "params": [[3, "_vue:x", None]]})
        else:
            vue_id += 1
            write_frame({"jsonrpc": "2.0", "method": "tsserver/request",
                         "params": [[vue_id, "_vue:projectInfo", {"file": uri}]]})
    elif "id" in msg:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}})
"#;

    /// Script Python factice aux « typescript-language-server » (tâche 05) :
    /// journalise dans argv[1] ; sur `workspace/executeCommand` avec
    /// `command == "typescript.tsserverRequest"`, répond l'OBJET RÉPONSE
    /// TSSERVER COMPLET dans `result` (`{type, success, body}`, vérifié sur
    /// TLS 6.0.0 — c'est le résultat LSP de la commande, PAS le `body` nu ;
    /// `body` porte `_cmd`/`_file` dérivés des `arguments` pour prouver
    /// l'appariement) ; sur `initialize`, répond `{"capabilities":{}}`.
    /// Modes via `fake_mode` : `"error"` (JSON-RPC erreur), `"null"`
    /// (`result: null` — sentinelle NoContent), `"swap"` (le 2e reçu répond
    /// avant le 1er — corrélation en désordre), `"die"` (meurt APRÈS avoir
    /// reçu la requête, sans répondre — purge d'EOF).
    const FAKE_LSP_TLS_PY: &str = r#"
import sys, json

LOG = sys.argv[1] if len(sys.argv) > 1 else ""
MODE = "@@MODE@@"

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

def tsserver_reply(msg):
    args = msg.get("params", {}).get("arguments", [])
    cmd = args[0] if len(args) > 0 else None
    payload = args[1] if len(args) > 1 else None
    file_ = payload.get("file") if isinstance(payload, dict) else None
    return {"jsonrpc": "2.0", "id": msg["id"],
            "result": {"type": "response", "success": True,
                       "body": {"configFileName": "/p/tsconfig.json",
                                "_cmd": cmd, "_file": file_}}}

pending_swap = []
while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(msg) + "\n")
    if "id" not in msg:
        continue
    is_fwd = (msg.get("method") == "workspace/executeCommand"
              and msg.get("params", {}).get("command") == "typescript.tsserverRequest")
    if not is_fwd:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
        continue
    if MODE == "error":
        write_frame({"jsonrpc": "2.0", "id": msg["id"],
                     "error": {"code": -32603, "message": "tsserver exploded"}})
    elif MODE == "null":
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif MODE == "swap":
        pending_swap.append(msg)
        if len(pending_swap) == 2:
            for m in reversed(pending_swap):
                write_frame(tsserver_reply(m))
            pending_swap = []
    elif MODE == "die":
        sys.exit(0)
    else:
        write_frame(tsserver_reply(msg))
"#;

    /// Script Python factice aux « saturé » (tâche 05) : reste vivant (stdout
    /// ouvert) mais ne lit JAMAIS stdin — le pipe puis le canal de la tâche
    /// écrivaine se remplissent, `try_send` vers l'aux rend `Full` alors même
    /// que l'aux est `alive`.
    const FAKE_LSP_SILENT_AUX_PY: &str = r#"
import time
while True:
    time.sleep(3600)
"#;

    /// Variante de comportement d'un fake (placeholder `@@MODE@@` de son
    /// script Python). C'est le FAKE qui a des modes — le contrat du
    /// multiplexeur, lui, est unique.
    fn fake_mode(script: &str, mode: &str) -> String {
        script.replace("@@MODE@@", mode)
    }

    /// Préambule commun des tests de forwarding : `initialize` jusqu'à la
    /// réponse du primaire (id restauré côté client).
    async fn composite_initialize(
        session: &LspSession,
        client: ClientId,
        rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("initialize ok");
        let resp = recv_timeout(rx, Duration::from_secs(5))
            .await
            .expect("réponse initialize du primaire");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(1)
        );
    }

    /// `didOpen` d'une URI : le fake primaire vue-ish émet un
    /// `tsserver/request` à chaque ouverture.
    async fn composite_open(session: &LspSession, client: ClientId, uri: &str) {
        let did_open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {"uri": uri, "languageId": "vue", "version": 1, "text": "<template/>"}
            }
        });
        session
            .send(client, did_open.to_string().into_bytes())
            .await
            .expect("didOpen ok");
    }

    /// Les `params` des trames `tsserver/response` reçues par un fake primaire
    /// (le multiplexeur émet `params = [[vue_id, body]]`).
    fn tsserver_response_params(frames: &[Value]) -> Vec<Value> {
        frames
            .iter()
            .filter(|f| f.get("method").and_then(|m| m.as_str()) == Some("tsserver/response"))
            .map(|f| f["params"].clone())
            .collect()
    }

    /// La trame `workspace/executeCommand typescript.tsserverRequest` reçue
    /// par un fake TLS, s'il y en a une.
    fn forwarded_execute_command(frames: &[Value]) -> Option<Value> {
        frames
            .iter()
            .find(|f| {
                f.get("method").and_then(|m| m.as_str()) == Some("workspace/executeCommand")
                    && f["params"]["command"].as_str() == Some("typescript.tsserverRequest")
            })
            .cloned()
    }

    /// Test 28: forward_project_info_roundtrip — (a) l'aux reçoit l'
    /// `executeCommand` EXACT (arguments = [command, payload], pas de
    /// ExecuteInfo), (b) le primaire reçoit `tsserver/response` avec le body
    /// DÉPILÉ de l'objet tsserver complet et le vue-id restitué, (c) le client
    /// ne voit AUCUNE trame tsserver/*.
    #[tokio::test]
    async fn forward_project_info_roundtrip() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", FAKE_LSP_TLS_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        // (a) l'aux reçoit l'executeCommand exact
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| forwarded_execute_command(frames).is_some(),
            Duration::from_secs(5),
        )
        .await;
        let exec = forwarded_execute_command(&aux_frames)
            .expect("l'aux doit recevoir l'executeCommand typescript.tsserverRequest");
        assert_eq!(
            exec["params"]["arguments"],
            serde_json::json!(["_vue:projectInfo", {"file": "file:///w/App.vue"}]),
            "arguments = [command, payload] exactement, pas de 3e ExecuteInfo; trames: {aux_frames:?}"
        );

        // (b) le primaire reçoit la tsserver/response [[1, body dépilé]]
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, {
                "configFileName": "/p/tsconfig.json",
                "_cmd": "_vue:projectInfo",
                "_file": "file:///w/App.vue"
            }]])],
            "params = [[vue_id, body DÉPILÉ]], id 1 restitué; trames: {primary_frames:?}"
        );

        // (c) le client ne voit aucune trame tsserver/* (ni request ni response)
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "une trame du dialogue interne tsserver/* a fuité vers le client: {leaked:?}"
        );

        drop(tmpdir);
    }

    /// Test 29: forward_aux_error_null_body — erreur JSON-RPC de l'aux ⟹
    /// primaire reçoit `[[id, null]]`, session vivante et fonctionnelle.
    #[tokio::test]
    async fn forward_aux_error_null_body() {
        let aux_script = fake_mode(FAKE_LSP_TLS_PY, "error");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, null]])],
            "erreur JSON-RPC de l'aux ⟹ body null; trames: {primary_frames:?}"
        );
        assert!(
            session.is_alive(),
            "une erreur de l'aux ne tue pas la session"
        );

        // session toujours fonctionnelle côté primaire
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("ping ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit répondre après une erreur de l'aux");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(2)
        );

        drop(tmpdir);
    }

    /// Test 30: forward_no_body_result_null — `result: null` (sentinelle
    /// NoContent de TLS, pas une erreur) ⟹ primaire reçoit `[[id, null]]`.
    #[tokio::test]
    async fn forward_no_body_result_null() {
        let aux_script = fake_mode(FAKE_LSP_TLS_PY, "null");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, null]])],
            "result null (NoContent) ⟹ body null; trames: {primary_frames:?}"
        );
        assert!(session.is_alive());

        drop(tmpdir);
    }

    /// Test 31: forward_aux_dead_null_body_no_hang — l'aux meurt (EOF) APRÈS
    /// avoir reçu la requête, sans jamais répondre ⟹ la purge des ids
    /// internes en attente à l'EOF doit répondre `[[id, null]]` au primaire ;
    /// session vivante, pas de pend.
    #[tokio::test]
    async fn forward_aux_dead_null_body_no_hang() {
        let aux_script = fake_mode(FAKE_LSP_TLS_PY, "die");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, null]])],
            "EOF aux avec requête en attente ⟹ purge body null; trames: {primary_frames:?}"
        );
        assert!(
            session.is_alive(),
            "la mort de l'aux après la requête ne tue pas la session"
        );

        drop(tmpdir);
    }

    /// Test 32: forward_no_alive_aux_null_body_immediate — aux mort avant
    /// même l'`initialize` : à l'émission `tsserver/request` du primaire, la
    /// réponse `[[id, null]]` part IMMÉDIATEMENT (poll borné à 2s — pas de
    /// pend, `vue-language-server` ne doit jamais attendre un aux absent).
    #[tokio::test]
    async fn forward_no_alive_aux_null_body_immediate() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", FAKE_LSP_EXIT_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        // Laisser l'aux rendre son EOF (exit immédiat) avant toute émission.
        tokio::time::sleep(Duration::from_millis(500)).await;

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, null]])],
            "aucun aux vivant ⟹ [[id, null]] quasi-immédiat, pas de pend; trames: {primary_frames:?}"
        );
        assert!(session.is_alive());

        drop(tmpdir);
    }

    /// Test 33: forward_concurrent_correlation — deux `tsserver/request`
    /// (ids 1, 2 via deux didOpen), l'aux répond EN ORDRE INVERSÉ (le 2e reçu
    /// répond avant le 1er) ⟹ les deux `tsserver/response` portent les bons
    /// vue_ids et les bons fichiers appariés (pas de croisement).
    #[tokio::test]
    async fn forward_concurrent_correlation() {
        let aux_script = fake_mode(FAKE_LSP_TLS_PY, "swap");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;
        composite_open(&session, client, "file:///w/Other.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| tsserver_response_params(frames).len() >= 2,
            Duration::from_secs(5),
        )
        .await;
        let responses = tsserver_response_params(&primary_frames);
        assert_eq!(
            responses.len(),
            2,
            "deux tsserver/response attendues; trames: {primary_frames:?}"
        );
        // L'aux a répondu en ordre inversé (le fake le garantit) : la première
        // réponse reçue par le primaire est celle du 2e request (id 2). La
        // corrélation par id interne doit malgré tout apparier id↔fichier.
        assert_eq!(
            responses[0],
            serde_json::json!([[2, {
                "configFileName": "/p/tsconfig.json",
                "_cmd": "_vue:projectInfo",
                "_file": "file:///w/Other.vue"
            }]]),
            "la réponse arrivée en premier doit être celle du 2e request (id 2, Other.vue)"
        );
        assert_eq!(
            responses[1],
            serde_json::json!([[1, {
                "configFileName": "/p/tsconfig.json",
                "_cmd": "_vue:projectInfo",
                "_file": "file:///w/App.vue"
            }]]),
            "la seconde réponse doit être celle du 1er request (id 1, App.vue)"
        );

        drop(tmpdir);
    }

    /// Test 34: forward_malformed_params_dropped — primaire émet
    /// `tsserver/request` avec `params: "garbage"` puis un bon
    /// `[[3, "_vue:x", null]]` : pas de panique, aucune réponse au garbage
    /// (id non exploitable), le payload `null` passe TEL QUEL dans
    /// `arguments`, et la réponse `[[3, null]]` (aux mode NoContent) atteint
    /// le primaire. Session OK.
    #[tokio::test]
    async fn forward_malformed_params_dropped() {
        let primary_script = fake_mode(FAKE_LSP_VUEISH_PY, "garbage");
        let aux_script = fake_mode(FAKE_LSP_TLS_PY, "null");
        let (spec, tmpdir) = make_fake_composite(
            primary_script.as_str(),
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;

        // Le payload `null` est passé tel quel dans les arguments.
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| forwarded_execute_command(frames).is_some(),
            Duration::from_secs(5),
        )
        .await;
        let exec = forwarded_execute_command(&aux_frames)
            .expect("le bon tsserver/request doit être forwardé malgré le garbage précédent");
        assert_eq!(
            exec["params"]["arguments"],
            serde_json::json!(["_vue:x", null]),
            "payload null passé tel quel dans arguments; trames: {aux_frames:?}"
        );

        // Exactement UNE tsserver/response (aucune pour le garbage, qui n'a
        // pas d'id exploitable) : [[3, null]] — aux mode NoContent.
        poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let primary_frames = read_recorder_frames(&primary_log_path(&tmpdir));
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[3, null]])],
            "exactement une réponse [[3, null]], jamais une pour le garbage; trames: {primary_frames:?}"
        );

        // Pas de panique, session vivante et fonctionnelle.
        assert!(session.is_alive());
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("ping ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("la session doit rester fonctionnelle après une trame malformée");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(2)
        );

        drop(tmpdir);
    }

    /// Test 34b: forward_aux_saturated_channel_null_body — aux VIVANT mais
    /// qui ne lit jamais stdin : pipe puis canal saturés ⟹ `try_send` plein ⟹
    /// `[[id, null]]` immédiat au primaire (l'aux est dégradé, la requête
    /// n'aurait jamais de réponse). Couvre la branche « canal plein » du
    /// fallback null-body.
    #[tokio::test]
    async fn forward_aux_saturated_channel_null_body() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUEISH_PY,
            &[("tsserver-forward", FAKE_LSP_SILENT_AUX_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;

        // Saturation : l'aux ne lit jamais — la tâche écrivaine se bloque sur
        // le pipe (64 Ko) et le canal (64 places) se remplit derrière. Marge
        // largement suffisante : 100 × 16 Ko ≫ pipe + canal.
        let pad = "x".repeat(16 * 1024);
        for i in 0..100 {
            let notif =
                serde_json::json!({"jsonrpc":"2.0","method":"fake/pad","params":{"i":i,"pad":pad}});
            session
                .send(client, notif.to_string().into_bytes())
                .await
                .expect("notification de saturation ok");
        }

        composite_open(&session, client, "file:///w/App.vue").await;

        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[1, null]])],
            "canal aux plein ⟹ body null immédiat, pas de pend; trames: {primary_frames:?}"
        );
        assert!(session.is_alive());

        drop(tmpdir);
    }
}
