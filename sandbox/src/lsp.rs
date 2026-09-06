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
    /// Contenu : le FONDU des parts par enfant (`diag_parts`) — plus jamais le
    /// dernier `publishDiagnostics` brut reçu. Mono-process (`aux` vide), une
    /// seule part possible : contenu exactement publié par le primaire,
    /// comportement historique inchangé.
    diagnostics_cache: Mutex<HashMap<String, Vec<Value>>>,
    /// Parts de diagnostics par URI et par enfant (0 = primaire, 1..N = index
    /// de `inner.aux`, porté par `AuxChild::index`). La concat dans l'ordre
    /// croissant des index produit la valeur fusionnée écrite dans
    /// `diagnostics_cache` AVANT broadcast — navigateur et MCP voient le même
    /// résultat (contrainte design n° 5). Publication d'un enfant =
    /// REMPLACEMENT de sa part (jamais append). Invalidée avec le cache,
    /// purgée à l'EOF de l'enfant.
    diag_parts: Mutex<HashMap<String, HashMap<usize, Vec<Value>>>>,
    diagnostics_notify: Notify,
    /// Enfants auxiliaires du multiplexeur (`spec.aux` spawnés, rôles connus).
    /// Vide ⟹ chemin mono-process strictement inchangé (invariant 8) : le
    /// primaire n'est PAS dans cette liste — il garde ses champs historiques
    /// ci-dessus (`cmd_tx`, `pending`, `next_req`, `child`).
    aux: Vec<Arc<AuxChild>>,
    /// Barrières de fusion des requêtes (tâche 07), clef sur le session id du
    /// primaire (espace d'ids historique de `pending`). Vide en mono-process
    /// (`aux: []`) comme pour toute méthode `MergeRoute::Primary` : le chemin
    /// historique n'est jamais dévié par une barrière inexistante (invariant
    /// 8). Purge par `unsubscribe` côté client.
    merges: Mutex<HashMap<u64, PendingMerge>>,
    /// Tâche 08 — cache de provenance des complétions SERVIS, pour
    /// `completionItem/resolve` : uri (telle que vue sur le fil de la session
    /// — mêmes clés que les notifications didChange/didClose) → ( (label,kind)
    /// → (index_enfant, item_original) ). Écrit à la complétion de chaque
    /// barrière completion (parts consignées APRÈS dédup — uniquement les
    /// items servis), purgé sur didChange/didClose de l'URI et à la saturation
    /// de la map. CACHE BORNÉ HEURISTIQUE (cap `MAX_COMPLETION_PROVENANCE_URIS`,
    /// éviction QUELCONQUE — ordre HashMap) : un faux fallback resolve no-op
    /// (item renvoyé tel quel) est l'échec ACCEPTABLE de ce cache, pas une
    /// erreur — l'important est qu'un resolve ne parte jamais au mauvais
    /// serveur. Jamais écrit ni consulté en mono-process (`aux` vide, les
    /// serveurs mono ont leur propre mémoire d'items — invariant 8).
    completion_provenance: Mutex<HashMap<String, CompletionProvenanceInner>>,
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
    /// Réponse d'un aux à une requête CLIENT fan-out (barrière task-07) :
    /// `session_id` est le session id du primaire (clef de `merges`) — l'id
    /// interne de l'aux ne sort jamais côté client, sa réponse est consignée
    /// comme part de la barrière (règle 3).
    ClientMerge { session_id: u64 },
    /// `completionItem/resolve` ROUTÉ vers cet enfant car c'est lui qui a
    /// fourni l'item (provenance task-08) : `client`/`orig_id` portent la
    /// réponse au client (id restauré) avec le RÉSULTAT de l'enfant. `item` =
    /// payload reçu du client, conservé pour le fallback no-op : erreur de
    /// l'enfant ou EOF avant réponse ⟹ réponse `{result: item}` (jamais une
    /// erreur globale, jamais un pend).
    ClientResolve {
        client: ClientId,
        orig_id: i64,
        item: Value,
    },
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
    /// Index d'enfant dans le multiplexeur : position dans `inner.aux` + 1 (0
    /// = primaire réservé), posé au pré-pass de `LspSession::spawn`. Porte ses
    /// parts de diagnostics dans `diag_parts` — la concat primary-first des
    /// index croissants produit le fondu (tâche 06). Index dans la liste des
    /// enfants RÉELLEMENT spawnés, pas dans `spec.aux` : rôle inconnu ignoré
    /// ou spawn en échec ne décale rien pour les enfants retenus.
    index: usize,
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
/// une fois la session Arc constituée. `index` : index d'enfant attribué par
/// le pré-pass (position dans la liste spawnée + 1, 0 = primaire) — porté par
/// `AuxChild` pour la fusion des diagnostics (tâche 06).
async fn spawn_aux_startup(
    aux_spec: &LspAux,
    sandbox_root: &Path,
    index: usize,
) -> anyhow::Result<AuxStartup> {
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
        index,
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

/// Concat des parts de diagnostics d'une URI dans l'ordre croissant des index
/// d'enfant (0 = primaire d'abord, puis aux dans l'ordre de `inner.aux`) —
/// déterminisme du fondu : indépendant de l'ordre d'arrivée des publications
/// (primary-first, contrainte design n° 5 de la tâche 06).
fn concat_diag_parts(uri_parts: &HashMap<usize, Vec<Value>>) -> Vec<Value> {
    let mut indexes: Vec<usize> = uri_parts.keys().copied().collect();
    indexes.sort_unstable();
    let mut merged = Vec::new();
    for index in indexes {
        if let Some(part) = uri_parts.get(&index) {
            merged.extend(part.iter().cloned());
        }
    }
    merged
}

// ── Tâche 07 : routage des requêtes en composite + fusions JSON pures ──────

/// Index de la part du PRIMAIRE dans une barrière `PendingMerge` (les aux
/// portent leur `AuxChild::index`, 1..N — 0 est réservé au primaire, comme
/// pour les parts de diagnostics de la tâche 06).
const PRIMARY_PART: usize = 0;

/// Cap du cache de provenance completion (tâche 08) : 64 URIs ; au-delà,
/// éviction QUELCONQUE (ordre `HashMap`) — cf. doc du champ
/// `completion_provenance` : un faux fallback resolve no-op est l'échec
/// acceptable de ce cache borné heuristique, pas une erreur.
const MAX_COMPLETION_PROVENANCE_URIS: usize = 64;

/// Table de provenance d'UNE URI (tâche 08) : `(label, kind)` →
/// `(index_enfant, item_original)` — alias posé pour la lisibilité (lint
/// `type_complexity`), type exactement celui du contrat de la tâche.
type CompletionProvenanceInner = HashMap<(String, Option<i64>), (usize, Value)>;

/// Politique de routage d'une REQUÊTE client en composite (`aux` non vide).
/// `aux` vide ⟹ la table n'est JAMAIS consultée, chemin primaire historique
/// strict (invariant 8). Défaut : `Primary` — méthodes mono-serveur par
/// design : `textDocument/formatting`, `textDocument/documentSymbol`,
/// `textDocument/signatureHelp` (non listée au design ⟹ défaut assumé,
/// extensible après vérification manuelle). Les méthodes `semanticTokens`
/// (full/range/delta + refresh) sont `Primary` par cas EXPLICITES de la table
/// (tâche 09 — jamais de fusion de flux de tokens, cf. commentaire du
/// `match`). Hors table par nature : `completionItem/resolve` — ni `Primary`
/// ni barrière, troisième voie dédiée par provenance
/// (`route_completion_resolve`, tâche 08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeRoute {
    Primary,
    All,
}

fn merge_route_for_method(method: &str) -> MergeRoute {
    match method {
        "textDocument/hover"
        | "textDocument/definition"
        | "textDocument/typeDefinition"
        | "textDocument/implementation"
        | "textDocument/references"
        | "textDocument/codeAction"
        | "textDocument/prepareRename"
        | "textDocument/rename"
        | "textDocument/completion" => MergeRoute::All,
        // semanticTokens/* : JAMAIS en fusion — deux flux de tokens sur un même
        // document ne sont pas concaténables (offsets absolus + legend). La
        // stratégie « délégation par plage » (décision 2026-09-06, risque n° 1)
        // est portée par le PRIMAIRE lui-même : il calcule template/style et
        // demande script → tsserver via `_vue:encodedSemanticClassifications-full`
        // en tsserver/request (canal tâche 05). Le multiplexeur ne voit qu'un flux
        // déjà complet, enrichi par l'aux via forwarding — routage primaire
        // EXPLICITE, pas un accident du défaut.
        "textDocument/semanticTokens/full"
        | "textDocument/semanticTokens/range"
        | "textDocument/semanticTokens/delta"
        | "workspace/semanticTokens/refresh" => MergeRoute::Primary,
        _ => MergeRoute::Primary,
    }
}

/// Warn sur une part de fusion au JSON inattendu : sa contribution devient
/// vide — JAMAIS une panique (contrat des fusionneurs, tâche 07).
fn warn_unexpected_merge_part(method: &str, part_index: usize, part: &Value) {
    tracing::warn!(
        method,
        part_index,
        "LSP: unexpected JSON in merge part, contributing empty: {part}"
    );
}

/// Une réponse `Hover.contents` est-elle un `MarkupContent` (champs `kind` et
/// `value` tous deux des chaînes) ? Les autres formes (`MarkedString` chaîne,
/// `{language, value}`, tableau) relèvent du repli de normalisation.
fn is_markup_content(contents: &Value) -> bool {
    contents.get("kind").and_then(Value::as_str).is_some()
        && contents.get("value").and_then(Value::as_str).is_some()
}

/// `hover` : `null | {contents}` ; tous null/absents ⟹ `null` ; un seul ⟹
/// celui-là verbatim ; plusieurs ⟹ si chaque `contents` est un
/// `MarkupContent` : valeur concaténée primary-first séparée par
/// `"\n\n---\n\n"` dans l'objet du primaire (kind = celui du primaire, champs
/// secondaires comme `range` conservés) ; SINON repli : normalisation en
/// TABLEAU de marked strings (chaîne ou objet) concaténé primary-first. Une
/// part non-objet ou sans `contents` exploitable contribue vide (warn).
fn merge_hover(parts: &[Option<Value>]) -> Value {
    let mut present: Vec<(usize, Value)> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            None | Some(Value::Null) => {}
            Some(Value::Object(hover)) => present.push((index, Value::Object(hover.clone()))),
            Some(unexpected) => warn_unexpected_merge_part("textDocument/hover", index, unexpected),
        }
    }
    match present.len() {
        0 => Value::Null,
        1 => present.remove(0).1,
        _ => {
            if present
                .iter()
                .all(|(_, hover)| hover.get("contents").is_some_and(is_markup_content))
            {
                let mut merged = present[0].1.clone();
                let joined = present
                    .iter()
                    .map(|(_, hover)| {
                        hover["contents"]["value"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect::<Vec<String>>()
                    .join("\n\n---\n\n");
                merged["contents"]["value"] = Value::String(joined);
                merged
            } else {
                let mut contents: Vec<Value> = Vec::new();
                for (index, hover) in &present {
                    match hover.get("contents") {
                        Some(Value::String(s)) => contents.push(Value::String(s.clone())),
                        Some(Value::Array(items)) => contents.extend(items.iter().cloned()),
                        Some(object @ Value::Object(_)) => contents.push(object.clone()),
                        _ => warn_unexpected_merge_part("textDocument/hover", *index, hover),
                    }
                }
                let mut merged = present[0].1.clone();
                merged["contents"] = Value::Array(contents);
                merged
            }
        }
    }
}

/// `definition` / `typeDefinition` / `implementation` / `references` : chaque
/// résultat `null`/absent | objet `Location` seul (wrap en tableau) | tableau
/// (`Location` ou `LocationLink`, mélangé accepté, opaque) ⟹ concaténation
/// primary-first ; toutes null ⟹ `null` ; sinon tableau (éventuellement
/// vide — un serveur qui répond `[]` a répondu). Une part scalaire inattendue
/// contribue vide (warn).
fn merge_locations(parts: &[Option<Value>]) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    let mut saw_locations = false;
    for (index, part) in parts.iter().enumerate() {
        match part {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                saw_locations = true;
                merged.extend(items.iter().cloned());
            }
            Some(value @ Value::Object(_)) => {
                saw_locations = true;
                merged.push(value.clone());
            }
            Some(unexpected) => warn_unexpected_merge_part("locations", index, unexpected),
        }
    }
    if saw_locations {
        Value::Array(merged)
    } else {
        Value::Null
    }
}

/// `textDocument/codeAction` : tableaux concaténés primary-first ; `null` ⟹
/// `[]` (la fusion de codeActions est TOUJOURS un tableau) ; non-tableau
/// inconnu ⟹ contribution vide + warn (jamais de panique).
fn merge_code_actions(parts: &[Option<Value>]) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => merged.extend(items.iter().cloned()),
            Some(unexpected) => {
                warn_unexpected_merge_part("textDocument/codeAction", index, unexpected)
            }
        }
    }
    Value::Array(merged)
}

/// `textDocument/prepareRename` : primaire non-`null` gagne ; sinon premier
/// non-`null` aux (fallback) ; tous null/absents ⟹ `null`.
fn merge_prepare_rename(parts: &[Option<Value>]) -> Value {
    parts
        .iter()
        .flatten()
        .find(|value| !value.is_null())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Variante `WorkspaceEdit` d'un objet : `changes` est la variante
/// prépondérante (la branche `documentChanges` ne s'applique qu'en son
/// absence — contrat tâche 07 « les DEUX avec documentChanges (et pas
/// changes) »).
fn workspace_edit_variant(edit: &serde_json::Map<String, Value>) -> Option<&'static str> {
    if edit.contains_key("changes") {
        Some("changes")
    } else if edit.contains_key("documentChanges") {
        Some("documentChanges")
    } else {
        None
    }
}

/// Pli gauche d'un `WorkspaceEdit` entrant dans le fold primary-first de
/// `textDocument/rename` : `changes` × `changes` ⟹ merge objet par URI
/// (concaténation des tableaux d'edits primary-first dans le MÊME tableau,
/// URIs disjointes ajoutées — une valeur non-tableau côté primaire n'est
/// jamais écrasée) ; `documentChanges` × `documentChanges` ⟹ concat des
/// tableaux primary-first ; variantes MIXTES (`changes` d'un côté,
/// `documentChanges` de l'autre) ⟹ on garde la variante du PRIMAIRE et les
/// edits de l'aux sont abandonnés avec `tracing::warn!` explicite (documenté
/// : rare, ne peut pas se mixer proprement, mieux vaut un renommage partiel
/// qu'un échec) ; entrant sans variante ⟹ rien à prendre. Champs secondaires
/// (`changeAnnotation`, metadata) : jamais copiés de l'aux — seuls ceux du
/// primaire survivent, le pli ne touche que les deux variantes d'edits.
fn fold_workspace_edit(
    mut held: serde_json::Map<String, Value>,
    incoming: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    match (
        workspace_edit_variant(&held),
        workspace_edit_variant(incoming),
    ) {
        (Some("changes"), Some("changes")) => {
            let held_ok = if let Some(held_changes) =
                held.get_mut("changes").and_then(Value::as_object_mut)
            {
                match incoming.get("changes").and_then(Value::as_object) {
                    Some(incoming_changes) => {
                        for (uri, edits) in incoming_changes {
                            match held_changes.get_mut(uri) {
                                Some(Value::Array(held_edits)) => match edits.as_array() {
                                    Some(incoming_edits) => {
                                        held_edits.extend(incoming_edits.iter().cloned());
                                    }
                                    None => warn_unexpected_merge_part(
                                        "textDocument/rename",
                                        usize::MAX,
                                        edits,
                                    ),
                                },
                                Some(existing) => warn_unexpected_merge_part(
                                    "textDocument/rename",
                                    usize::MAX,
                                    existing,
                                ),
                                None => {
                                    held_changes.insert(uri.clone(), edits.clone());
                                }
                            }
                        }
                        true
                    }
                    None => false,
                }
            } else {
                false
            };
            if !held_ok {
                tracing::warn!("LSP: rename changes merge on unexpected JSON, keeping primary's");
            }
        }
        (Some("documentChanges"), Some("documentChanges")) => {
            let merged = match (
                held.get_mut("documentChanges")
                    .and_then(Value::as_array_mut),
                incoming.get("documentChanges").and_then(Value::as_array),
            ) {
                (Some(held_changes), Some(incoming_changes)) => {
                    held_changes.extend(incoming_changes.iter().cloned());
                    true
                }
                _ => false,
            };
            if !merged {
                tracing::warn!(
                    "LSP: rename documentChanges merge on unexpected JSON, keeping primary's"
                );
            }
        }
        (Some(held_variant), Some(incoming_variant)) => {
            // Variants identiques => deja mergees plus haut ; ici c'est le
            // cas MIXTE : garde la variante primaire, abandon explicite.
            tracing::warn!(
                held_variant,
                incoming_variant,
                "LSP: rename WorkspaceEdit mixed variants, keeping primary's and dropping aux's edits"
            );
        }
        (None, Some(variant)) => {
            // Primaire sans variante (WorkspaceEdit de champs secondaires
            // seuls) : on adopte celle de l'aux.
            if let Some(value) = incoming.get(variant) {
                held.insert(variant.to_string(), value.clone());
            }
        }
        _ => {}
    }
    held
}

/// `textDocument/rename` : `null | WorkspaceEdit` ; un seul ⟹ celui-là ;
/// plusieurs ⟹ pli `fold_workspace_edit` primary-first. Parts null/absentes
/// ignorées ; non-objet inattendu ⟹ contribution vide + warn.
fn merge_rename(parts: &[Option<Value>]) -> Value {
    let mut acc: Option<serde_json::Map<String, Value>> = None;
    for (index, part) in parts.iter().enumerate() {
        let Some(value) = part.as_ref().filter(|value| !value.is_null()) else {
            continue;
        };
        let Some(edit) = value.as_object() else {
            warn_unexpected_merge_part("textDocument/rename", index, value);
            continue;
        };
        acc = Some(match acc.take() {
            None => edit.clone(),
            Some(held) => fold_workspace_edit(held, edit),
        });
    }
    acc.map(Value::Object).unwrap_or(Value::Null)
}

/// Clé de dédup ET de provenance d'un `CompletionItem` (tâche 08) :
/// `(label, kind)` — `label` = la chaîne LSP classique ou l'objet
/// `CompletionItemLabel` LSP 3.17 (`label.label`) ; `kind` = `kind` (i64) ou
/// `None` si absent — **absent ≠ présent** (décision développeur 2026-09-06).
/// `None` = item sans label exploitable (opaque) : jamais dédupé (conservé
/// tel quel), jamais traçable par la provenance — un resolve dessus retombera
/// en fallback no-op, l'échec acceptable.
fn completion_item_key(item: &Value) -> Option<(String, Option<i64>)> {
    let label = match item.get("label")? {
        Value::String(s) => s.clone(),
        Value::Object(label_obj) => label_obj.get("label")?.as_str()?.to_string(),
        _ => return None,
    };
    Some((label, item.get("kind").and_then(Value::as_i64)))
}

/// Cœur de la fusion completion (tâche 08) : normalise chaque part
/// (`null`/absent ⟹ vide ; tableau ⟹ ses items ; objet avec `items` tableau
/// ⟹ `CompletionList` : items + `isIncomplete`) et la plie primary-first dans
/// `retained` — un item dont `(label, kind)` matche un item DÉJÀ RETENU est
/// abandonné (le retenu est le plus ancien ⟹ le primaire gagne), un item sans
/// clé est conservé sans être dédupable. Parts indexées par index d'enfant
/// (0 = primaire) pour que la provenance sache QUI a fourni chaque item
/// retenu ; les positions du vecteur d'entrée portent déjà l'ordre
/// primary-first. Part d'une shape inattendue (scalaire, objet sans `items`)
/// ⟹ contribution vide + warn (contrat des fusionneurs, jamais de panique).
/// Rend `(isIncomplete en OR des parts, items retenus avec leur index
/// d'enfant, au moins une part était un CompletionList)`.
fn merge_completion_parts(parts: &[(usize, Option<Value>)]) -> (bool, Vec<(usize, Value)>, bool) {
    let mut seen: HashSet<(String, Option<i64>)> = HashSet::new();
    let mut retained: Vec<(usize, Value)> = Vec::new();
    let mut is_incomplete = false;
    let mut any_completion_list = false;
    for (index, part) in parts {
        let items: &[Value] = match part {
            None | Some(Value::Null) => continue,
            Some(Value::Array(items)) => items.as_slice(),
            Some(Value::Object(list)) => match list.get("items").and_then(Value::as_array) {
                Some(items) => {
                    any_completion_list = true;
                    if list
                        .get("isIncomplete")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        is_incomplete = true;
                    }
                    items.as_slice()
                }
                None => {
                    warn_unexpected_merge_part(
                        "textDocument/completion",
                        *index,
                        &Value::Object(list.clone()),
                    );
                    continue;
                }
            },
            Some(unexpected) => {
                warn_unexpected_merge_part("textDocument/completion", *index, unexpected);
                continue;
            }
        };
        for item in items {
            let key = completion_item_key(item);
            if let Some(key) = &key
                && !seen.insert(key.clone())
            {
                continue; // le primaire (ou un enfant plus tôt) a déjà gagné pour cette clé
            }
            retained.push((*index, item.clone()));
        }
    }
    (is_incomplete, retained, any_completion_list)
}

/// `textDocument/completion` (fusion de barrière, tâche 08) : `merge_result_for`
/// ne connaît que les valeurs des parts — cœur partagé avec l'écriture de la
/// provenance (qui, elle, a besoin des index d'enfants) : même fonction,
/// indices de position ici. Shape de sortie : au moins une part `CompletionList`
/// ⟹ `CompletionList {isIncomplete, items}` ; sinon (toutes tableaux/null) ⟹
/// le tableau — préserver la compat d'attente des deux mondes clientes
/// (`@codemirror/lsp-client` lit les deux).
fn merge_completions(parts: &[Option<Value>]) -> Value {
    let indexed: Vec<(usize, Option<Value>)> = parts.iter().cloned().enumerate().collect();
    let (is_incomplete, retained, any_completion_list) = merge_completion_parts(&indexed);
    let items: Vec<Value> = retained.into_iter().map(|(_, item)| item).collect();
    if any_completion_list {
        serde_json::json!({"isIncomplete": is_incomplete, "items": items})
    } else {
        Value::Array(items)
    }
}

/// Dispatch de la fusion vers les cas de la table `MergeRoute::All` (table
/// fermée : toute méthode `All` a son fusionneur). Méthode inconnue
/// (impossible par la table, défense) ⟹ part du primaire ou `null`, warn —
/// jamais de panique.
fn merge_result_for(method: &str, parts: &[Option<Value>]) -> Value {
    match method {
        "textDocument/hover" => merge_hover(parts),
        "textDocument/definition"
        | "textDocument/typeDefinition"
        | "textDocument/implementation"
        | "textDocument/references" => merge_locations(parts),
        "textDocument/codeAction" => merge_code_actions(parts),
        "textDocument/prepareRename" => merge_prepare_rename(parts),
        "textDocument/rename" => merge_rename(parts),
        "textDocument/completion" => merge_completions(parts),
        unknown => {
            tracing::warn!(
                method = unknown,
                "LSP: no merge function for routed method, keeping primary's part"
            );
            parts
                .first()
                .and_then(|part| part.clone())
                .filter(|value| !value.is_null())
                .unwrap_or(Value::Null)
        }
    }
}

/// Une entrée de barrière (tâche 07) : la requête d'un client fan-out vers le
/// primaire (part 0) + chaque aux vivant (parts 1..N, index
/// `AuxChild::index`), UNE réponse client unique à la fin. `expected` =
/// nombre de parts réellement émises (aux morts/injoignables exclus — cf.
/// règle 1). La barrière est clef sur le SESSION id du primaire : le primaire
/// garde son espace d'ids historique (`next_req`/`pending` inchangés), les
/// aux répondent sous leurs ids internes corrélés par
/// `InternalPending::ClientMerge`. Une seule réponse sortante par requête
/// client : `result` fusionné primary-first (ou `error` du primaire transmis
/// tel quel en échec fast), id d'origine restauré.
struct PendingMerge {
    client: ClientId,
    orig_id: i64,
    method: String,
    /// `params.textDocument.uri` de la requête, si présent (tâche 08) : seul
    /// consommateur = l'écriture de la provenance à la complétion d'une
    /// barrière `completion`. Absent ⟹ pas de provenance (un resolve
    /// correspondant retombera en fallback no-op — acceptable, documenté).
    params_uri: Option<String>,
    expected: usize,
    /// part index (0 = primaire) → réponse ; `None` = l'enfant a rendu une
    /// erreur/EOF (contribution vide — seul le primaire en ERREUR fait échouer
    /// la requête entière, règles 4–5 tâche 07).
    parts: HashMap<usize, Option<Value>>,
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
        // Index d'enfant porté par chaque enfant retenu : position dans la
        // liste spawnée + 1 (0 réservé au primaire) — fusion des diagnostics,
        // tâche 06.
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
            match spawn_aux_startup(aux_spec, sandbox_root, aux_startups.len() + 1).await {
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
                diag_parts: Mutex::new(HashMap::new()),
                diagnostics_notify: Notify::new(),
                child: Mutex::new(Some(child)),
                next_client: AtomicU64::new(1),
                next_req: AtomicU64::new(1),
                aux,
                merges: Mutex::new(HashMap::new()),
                completion_provenance: Mutex::new(HashMap::new()),
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
                        // Tâche 07 : une réponse du primaire sur une session
                        // id gouvernée par une barrière (`Route::All` en
                        // composite) est une PART de fusion, jamais une
                        // réponse client directe. Hors barrière (mono-process
                        // inclus) ⟹ chemin historique exactement en dessous,
                        // sans un octet de changement (règle 8, invariant 8).
                        if reader_session.record_primary_merge_part(session_id, &msg) {
                            continue;
                        }
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
                        // (tâche 05) ; toute autre notification : dispatch
                        // partagé avec l'index du primaire (0) — fusion
                        // `publishDiagnostics` par parts AVANT le cache, puis
                        // broadcast du même fondu (tâche 06).
                        if msg.get("method").and_then(|m| m.as_str()) == Some("tsserver/request") {
                            reader_session.handle_tsserver_request(&msg);
                        } else {
                            reader_session.dispatch_notification_frame(&msg, payload, 0);
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
                                Some(InternalPending::ClientMerge { session_id }) => {
                                    // Tâche 07, règle 3 : réponse de l'aux à
                                    // une requête CLIENT fan-out — consignée
                                    // dans la barrière (résultat →
                                    // `Some(result)`, erreur → `None` + warn
                                    // dégradé, jamais une erreur globale). La
                                    // réponse brute ne sort JAMAIS vers un
                                    // client ; barrière disparue ⟹ debug.
                                    reader_session.record_aux_merge_part(
                                        reader_aux.role.as_str(),
                                        reader_aux.index,
                                        session_id,
                                        &msg,
                                    );
                                }
                                Some(InternalPending::ClientResolve {
                                    client,
                                    orig_id,
                                    item,
                                }) => {
                                    // Tâche 08 : réponse de l'ENFANT D'ORIGINE
                                    // à un resolve routé par provenance —
                                    // résultat relayé au client (id d'origine
                                    // restauré). Erreur JSON-RPC (ou trame sans
                                    // résultat) ⟹ fallback item TEL QUEL +
                                    // warn : l'échec acceptable est le no-op,
                                    // jamais une erreur globale, jamais un
                                    // pend.
                                    if let Some(result) = msg.get("result") {
                                        reader_session.send_to_client(
                                            client,
                                            &serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": orig_id,
                                                "result": result.clone(),
                                            }),
                                        );
                                    } else {
                                        let error =
                                            msg.get("error").cloned().unwrap_or(Value::Null);
                                        tracing::warn!(
                                            toolchain =
                                                reader_session.inner._toolchain_name.as_str(),
                                            role = reader_aux.role.as_str(),
                                            "LSP: aux error on routed completionItem/resolve, serving item un-resolved (no-op fallback): {error}"
                                        );
                                        reader_session.send_to_client(
                                            client,
                                            &serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": orig_id,
                                                "result": item,
                                            }),
                                        );
                                    }
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
                            // Notification d'un enfant (aux inclus) : dispatch
                            // partagé avec SON index d'enfant (`AuxChild::index`)
                            // — fusion des parts de diagnostics avant le cache
                            // et broadcast du même fondu (tâche 06 ; l'index
                            // distingue ses parts de celles du primaire et des
                            // autres aux).
                            reader_session.dispatch_notification_frame(
                                &msg,
                                payload,
                                reader_aux.index,
                            );
                        }
                    }
                }

                // EOF stdout d'un AUX : la session SURVIT (`is_alive()` ne
                // reflète que le primaire). Aux baissé, parts de diagnostics
                // purgées, ids internes en attente purgés (ils n'auront jamais
                // de réponse) — les
                // `TsserverForward` reçoivent d'abord une `tsserver/response`
                // `[[vue_id, null]]` (best effort `try_send` : primaire mort ⟹
                // ignoré, la requête d'origine n'aurait de toute façon jamais
                // existé) pour que `vue-language-server` ne pende pas ; les
                // `Initialize` se font clear comme avant (tâche 03) ; les
                // `ClientMerge` (tâche 07, règle 5) lèvent leur part en `None`
                // avec tentative de complétion — un serveur mort ne doit pas
                // bloquer la fusion primaire. Et warn unique « degraded »
                // seulement si la session est encore vivante : si le primaire
                // vient de mourir en tuant cet aux, la dégradation serait du
                // bruit.
                reader_aux.alive.store(false, Ordering::SeqCst);
                // Purge des parts de CET enfant + recomputation du cache
                // (tâche 06). Justification de la purge plutôt que du maintien :
                // des diagnostics d'un serveur MORT qui resteraient publiés
                // feraient passer un `edit_and_check` au vert sur des erreurs
                // fantômes — disparaître est le comportement sûr. Pas de notify
                // sur ce chemin : réduction du cache, pas information nouvelle.
                reader_session.drop_aux_parts(reader_aux.index);
                let mut merge_session_ids: Vec<u64> = Vec::new();
                let mut resolve_fallbacks: Vec<(ClientId, i64, Value)> = Vec::new();
                {
                    let mut pending = reader_aux
                        .pending_internal
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for entry in pending.values() {
                        match entry {
                            InternalPending::TsserverForward { vue_id } => {
                                reader_session.send_tsserver_response(vue_id.clone(), Value::Null);
                            }
                            InternalPending::ClientMerge { session_id } => {
                                merge_session_ids.push(*session_id);
                            }
                            InternalPending::ClientResolve {
                                client,
                                orig_id,
                                item,
                            } => {
                                // Tâche 08 : ces resolve n'auront JAMAIS de
                                // réponse de cet enfant mort ⟹ fallback item
                                // tel quel (no-op), collecté pour l'envoi HORS
                                // du verrou (`send_to_client` prend `subs`).
                                resolve_fallbacks.push((*client, *orig_id, item.clone()));
                            }
                            InternalPending::Initialize => {}
                        }
                    }
                    pending.clear();
                }
                // Hors du verrou `pending_internal` : `insert_merge_part`
                // prend le verrou `merges` (jamais les deux simultanément).
                for session_id in merge_session_ids {
                    reader_session.insert_merge_part(reader_aux.index, session_id, None);
                }
                // Idem tâche 08 : réponse de fallback aux clients dont le
                // resolve est mort avec l'enfant (warn « degraded » porteur).
                for (client, orig_id, item) in resolve_fallbacks {
                    tracing::warn!(
                        toolchain = reader_session.inner._toolchain_name.as_str(),
                        role = reader_aux.role.as_str(),
                        orig_id,
                        "LSP: aux died mid completionItem/resolve, serving item un-resolved (no-op fallback)"
                    );
                    reader_session.send_to_client(
                        client,
                        &serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": orig_id,
                            "result": item,
                        }),
                    );
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

    /// Traite une trame NOTIFICATION (sans `id`) reçue d'un enfant — primaire
    /// (`child_idx = 0`) ou aux (son `AuxChild::index`) — chemin partagé par
    /// invariant 5. `publishDiagnostics` : la part de CET enfant REMPLACE la
    /// sienne dans `diag_parts`, le fondu concat primary-first est écrit dans
    /// `diagnostics_cache` AVANT tout broadcast — cache et broadcast portent
    /// le MÊME contenu en composite (navigateur et MCP consomment le même
    /// résultat, contrainte design n° 5) ; `aux` vide (mono-process, tout ce
    /// qui est déployé) ⟹ payload d'origine octet-identique (invariant 8).
    /// Toute autre notification : broadcast de la part brute, chemin actuel
    /// inchangé. Le cache alimente TOUT abonné, présent ou futur,
    /// indépendamment du broadcast ci-dessous (cf. doc du champ
    /// `diagnostics_cache`) ; nettoyage des canaux fermés au passage.
    ///
    /// La fusion des REQUÊTES (routage par méthode, barrière `PendingMerge`
    /// et ids internes d'aux — tâches 03/05/07) ne passe par CE chemin : elle
    /// vit dans `merge_route_for_method`/`send` (fan-out),
    /// `record_*_merge_part`/`complete_merge` (corrélation et réponse
    /// fusionnée unique). Rien à fusionner ici.
    fn dispatch_notification_frame(&self, msg: &Value, payload: Vec<u8>, child_idx: usize) {
        // Le payload diffusé aux abonnés : part brute par défaut (mono-process
        // — octets d'origine inchangés — et trames non-diagnostic), fondu
        // synthétisé en composite `publishDiagnostics` (cf. flux tâche 06).
        let mut broadcast_payload = payload;
        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics")
            && let Some(params) = msg.get("params")
            && let Some(uri) = params.get("uri").and_then(|u| u.as_str())
            && let Some(diags) = params.get("diagnostics").and_then(|d| d.as_array())
        {
            let merged = {
                let mut parts = self
                    .inner
                    .diag_parts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let uri_parts = parts.entry(uri.to_string()).or_default();
                uri_parts.insert(child_idx, diags.clone());
                let merged = concat_diag_parts(uri_parts);
                if let Ok(mut cache) = self.inner.diagnostics_cache.lock() {
                    cache.insert(uri.to_string(), merged.clone());
                }
                merged
            };
            self.inner.diagnostics_notify.notify_waiters();
            if !self.inner.aux.is_empty() {
                // Composite : les abonnés ne voient JAMAIS une part brute —
                // clone de la trame reçue avec `params.diagnostics` remplacé
                // par le fondu (les autres champs de `params` — `uri`,
                // `version` — conservés de la trame qui a déclenché).
                let mut synthesized = msg.clone();
                synthesized["params"]["diagnostics"] = Value::Array(merged);
                broadcast_payload = synthesized.to_string().into_bytes();
            }
        }

        // Notification (pas d'id) : broadcast à tous les abonnés
        let subs_map = match self.inner.subs.lock() {
            Ok(g) => g,
            Err(g) => g.into_inner(),
        };

        let dead_clients: Vec<_> = subs_map
            .iter()
            .filter(|(_client_id, tx)| {
                let raw_json = broadcast_payload.clone();
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

    /// EOF d'un aux (enfant `child_idx`) : retire ses parts de `diag_parts`
    /// pour TOUTES les URI puis recompte `diagnostics_cache[uri]` = concat des
    /// parts restantes ; une URI sans aucune part restante est retirée du
    /// cache. Pas de `notify_waiters` : réduction du cache, pas nouvelle
    /// information (justification de la purge : cf. commentaire de l'appelant
    /// dans la lectrice aux — des diagnostics d'un serveur mort qui resteraient
    /// publiés feraient passer un `edit_and_check` au vert sur des erreurs
    /// fantômes). Même ordre de verrous que `dispatch_notification_frame`
    /// (`diag_parts` puis `diagnostics_cache`) — pas d'inversion, pas
    /// d'interblocage.
    fn drop_aux_parts(&self, child_idx: usize) {
        let mut parts = self
            .inner
            .diag_parts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut cache = self
            .inner
            .diagnostics_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (uri, uri_parts) in parts.iter_mut() {
            if uri_parts.remove(&child_idx).is_none() {
                continue; // URI sans part de cet enfant : rien à recompter
            }
            if uri_parts.is_empty() {
                cache.remove(uri);
            } else {
                cache.insert(uri.clone(), concat_diag_parts(uri_parts));
            }
        }
        parts.retain(|_, uri_parts| !uri_parts.is_empty());
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

    /// Émet une trame vers un abonné (id restauré par l'appelant) : même
    /// motif que le routage historique de la lectrice primaire — client absent
    /// (déconnecté entre-temps) ⟹ ignoré silencieusement, JAMAIS de réponse
    /// vers un client désabonné (règle 7).
    fn send_to_client(&self, client: ClientId, msg: &Value) {
        if let Ok(subs) = self.inner.subs.lock()
            && let Some(tx) = subs.get(&client)
        {
            let _ = tx.send(msg.to_string().into_bytes());
        }
    }

    /// Tâche 07, règles 2–4 — réponse du PRIMAIRE portant un session id
    /// gouverné par une barrière : `rendre true` ⟹ la barrière possédait la
    /// trame (consignée en part 0 si `result`, barrière RETENUE tant qu'elle
    /// n'est pas complète, ou échec fast si `error`), le chemin historique
    /// `pending` NE doit PAS s'exécuter — la réponse brute du primaire ne
    /// part jamais telle quelle au client en mode fusion. `rendre false` ⟹
    /// aucune barrière (mono-process inclus) : le chemin historique
    /// s'exécute exactement comme avant (règle 8).
    fn record_primary_merge_part(&self, session_id: u64, msg: &Value) -> bool {
        let mut merges = self.inner.merges.lock().unwrap_or_else(|e| e.into_inner());
        if !merges.contains_key(&session_id) {
            return false;
        }
        // La barrière possède la requête : hygiène du `pending` (que le
        // chemin historique aurait nettoyée). Aucune inversion de verrous
        // possible : nulle part le verrou `merges` n'est pris PENDANT que
        // `pending` est tenu (`send` les séquence, `unsubscribe` aussi, la
        // lectrice aux n'a que `pending_internal`).
        if let Ok(mut pending) = self.inner.pending.lock() {
            pending.remove(&session_id);
        }
        if let Some(error) = msg.get("error") {
            // Règle 4 : erreur PRIMAIRE ⟹ réponse erreur immédiate au client
            // (id restauré, structure `error` transmise TELLE QUELLE),
            // barrière retirée — les parts aux qui arrivent après sont
            // ignorées silencieusement (entrée absente).
            if let Some(pending_merge) = merges.remove(&session_id) {
                tracing::warn!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    method = pending_merge.method.as_str(),
                    "LSP: primary error on merged request, failing fast: {error}"
                );
                drop(merges);
                self.send_to_client(
                    pending_merge.client,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": pending_merge.orig_id,
                        "error": error,
                    }),
                );
            }
            return true;
        }
        let part = match msg.get("result") {
            Some(result) => Some(result.clone()),
            None => {
                // Ni `result` ni `error` (trame inattendue) : contribution
                // vide, warn — la barrière se complétera sans le primaire.
                tracing::warn!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    session_id,
                    "LSP: primary response with neither result nor error, merge part empty"
                );
                None
            }
        };
        let completed = {
            let Some(pending) = merges.get_mut(&session_id) else {
                return true; // purgé entre le test et le get_mut (règle 7)
            };
            pending.parts.insert(PRIMARY_PART, part);
            if pending.parts.len() < pending.expected {
                return true;
            }
            merges.remove(&session_id)
        };
        drop(merges);
        if let Some(pending_merge) = completed {
            self.complete_merge(pending_merge);
        }
        true
    }

    /// Tâche 07, règle 3 — réponse d'un aux à une requête client fan-out :
    /// résultat → `Some(result)`, erreur JSON-RPC → `None` + warn dégradé
    /// (la fusion continue sans cette part — jamais une erreur globale), ni
    /// résultat ni erreur → `None` + warn.
    fn record_aux_merge_part(&self, role: &str, part_index: usize, session_id: u64, msg: &Value) {
        let part = if let Some(error) = msg.get("error") {
            tracing::warn!(
                toolchain = self.inner._toolchain_name.as_str(),
                role,
                "LSP: aux error on merged client request, empty part (degraded merge): {error}"
            );
            None
        } else {
            match msg.get("result") {
                Some(result) => Some(result.clone()),
                None => {
                    tracing::warn!(
                        toolchain = self.inner._toolchain_name.as_str(),
                        role,
                        session_id,
                        "LSP: aux response with neither result nor error, merge part empty"
                    );
                    None
                }
            }
        };
        self.insert_merge_part(part_index, session_id, part);
    }

    /// Consigne la part `part_index` de la barrière `session_id` (règles
    /// 3/5/6) et tente la complétion. Barrière disparue (déjà complétée,
    /// tuée par l'erreur primaire, ou purgée par `unsubscribe`) ⟹ debug +
    /// ignore — c'est le chemin normal des parts tardives.
    fn insert_merge_part(&self, part_index: usize, session_id: u64, part: Option<Value>) {
        let completed = {
            let mut merges = self.inner.merges.lock().unwrap_or_else(|e| e.into_inner());
            let Some(pending) = merges.get_mut(&session_id) else {
                tracing::debug!(
                    session_id,
                    part_index,
                    "LSP: merge part for absent barrier (completed, failed fast or unsubscribed), dropping"
                );
                return;
            };
            pending.parts.insert(part_index, part);
            if pending.parts.len() < pending.expected {
                return;
            }
            merges.remove(&session_id)
        };
        if let Some(pending_merge) = completed {
            self.complete_merge(pending_merge);
        }
    }

    /// Tente la complétion de la barrière `session_id` sans consigner de part
    /// — pour les chemins où `expected` BAISSE après la dernière réponse
    /// attendue (exclusion d'un aux injoignable dans `send`).
    fn try_complete_merge(&self, session_id: u64) {
        let completed = {
            let mut merges = self.inner.merges.lock().unwrap_or_else(|e| e.into_inner());
            match merges.get(&session_id) {
                Some(pending) if pending.parts.len() >= pending.expected => {
                    merges.remove(&session_id)
                }
                _ => None,
            }
        };
        if let Some(pending_merge) = completed {
            self.complete_merge(pending_merge);
        }
    }

    /// Règle 1 — l'aux `role` ne recevra jamais cette requête (injoignable :
    /// `try_send` en échec/fermé, ou mort entre le comptage et l'envoi) ⟹ sa
    /// part est supprimée du compte : `expected -= 1`, jamais d'attente
    /// perpétuelle. Barrière déjà disparue (erreur primaire fast, purge) ⟹
    /// rien à décrémenter.
    fn abandon_merge_expectation(&self, session_id: u64, role: &str) {
        if let Some(pending_merge) = self
            .inner
            .merges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&session_id)
        {
            pending_merge.expected = pending_merge.expected.saturating_sub(1);
        }
        tracing::debug!(
            toolchain = self.inner._toolchain_name.as_str(),
            role,
            session_id,
            "LSP: aux degraded (cannot receive merged request), part dropped"
        );
    }

    /// Règle 6 — barrière complète (`parts.len() == expected`) : parties
    /// index-ordered (0 = primaire d'abord, indépendamment de l'ordre
    /// d'arrivée), fusion `merge_result_for`, puis LA réponse client unique
    /// (id d'origine restauré, résultat fusionné primary-first). Les réponses
    /// brutes d'enfants ne sortent jamais. `send_to_client` filtre les
    /// désabonnés (règle 7).
    ///
    /// Tâche 08 : une barrière `completion` complétée écrit en plus la
    /// provenance des items SERVIS (même cœur `merge_completion_parts` que la
    /// fusion ⟹ consistance résultat/provenance par construction).
    fn complete_merge(&self, pending_merge: PendingMerge) {
        let mut indexes: Vec<usize> = pending_merge.parts.keys().copied().collect();
        indexes.sort_unstable();
        let parts: Vec<Option<Value>> = indexes
            .iter()
            .filter_map(|index| pending_merge.parts.get(index).cloned())
            .collect();
        if pending_merge.method == "textDocument/completion" {
            self.record_completion_provenance(&pending_merge, &indexes, &parts);
        }
        let result = merge_result_for(&pending_merge.method, &parts);
        self.send_to_client(
            pending_merge.client,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": pending_merge.orig_id,
                "result": result,
            }),
        );
    }

    /// Tâche 08 — écrit la provenance des items SERVIS par une barrière
    /// completion complétée : `(label,kind) → (index_enfant, item_original)`
    /// pour l'URI de la requête, APRÈS dédup (les items droppés n'ont jamais
    /// été vus d'un éditeur — jamais de provenance sur eux), REMPLACEMENT de
    /// l'entrée existante de l'URI (la dernière complétion fait foi). Cap
    /// `MAX_COMPLETION_PROVENANCE_URIS` : au-delà, éviction QUELCONQUE (ordre
    /// `HashMap`) — cache borné heuristique, un faux fallback resolve no-op
    /// est l'échec acceptable, pas l'erreur (cf. doc du champ). Requête sans
    /// `params.textDocument.uri` ⟹ rien à tracer (debug) : le resolve
    /// correspondant retombera en no-op — documenté, acceptable.
    fn record_completion_provenance(
        &self,
        pending_merge: &PendingMerge,
        indexes: &[usize],
        parts: &[Option<Value>],
    ) {
        let Some(uri) = pending_merge.params_uri.as_deref() else {
            tracing::debug!(
                toolchain = self.inner._toolchain_name.as_str(),
                session_id = pending_merge.orig_id,
                "LSP: merged completion without params.textDocument.uri, provenance not recorded"
            );
            return;
        };
        let indexed: Vec<(usize, Option<Value>)> =
            indexes.iter().copied().zip(parts.iter().cloned()).collect();
        let (_, retained, _) = merge_completion_parts(&indexed);
        let mut inner: CompletionProvenanceInner = HashMap::new();
        for (child_index, item) in retained {
            if let Some(key) = completion_item_key(&item) {
                inner.insert(key, (child_index, item));
            }
        }
        let mut provenance = self
            .inner
            .completion_provenance
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !provenance.contains_key(uri) && provenance.len() >= MAX_COMPLETION_PROVENANCE_URIS {
            let evicted = provenance.keys().next().cloned();
            if let Some(evicted) = evicted {
                provenance.remove(&evicted);
            }
        }
        provenance.insert(uri.to_string(), inner);
    }

    /// Tâche 08 — troisième voie de `completionItem/resolve` en composite :
    /// **jamais** de barrière, **jamais** le primaire aveugle. Clé
    /// `(label, kind)` de l'item reçu (`params` du client — même forme qu'un
    /// `CompletionItem`), cherchée dans TOUTES les URI du cache de
    /// provenance → ensemble des enfants distincts fournisseurs :
    ///
    /// - un seul enfant PRIMAIRE (index 0) ⟹ `false` : le chemin historique
    ///   de `send` EST le routage primaire requis (session id réécrit,
    ///   `pending`, réponse du primaire id restauré) — aucune mécanique
    ///   nouvelle, aucune trame surnuméraire ;
    /// - un seul enfant AUX ⟹ copie de la requête avec SON id interne +
    ///   `InternalPending::ClientResolve` (enregistré AVANT envoi, même motif
    ///   que le fan-out task-07) ; réponse de l'enfant ⟹ réponse au client
    ///   (id restauré) avec le RÉSULTAT de l'enfant ; injoignable ou mort ⟹
    ///   warn + fallback item tel quel ;
    /// - zéro correspondance ou plusieurs enfants distincts (ambigu) ⟹
    ///   fallback immédiat `{result: item}`, AUCUNE trame sur les fils
    ///   enfants — le resolve ne devient jamais une erreur globale, le no-op
    ///   est l'échec acceptable.
    ///
    /// Rend `true` ⟹ prise en charge (routée aux ou fallback) ; `false` ⟹
    /// laisser passer au chemin historique (propriétaire = primaire).
    fn route_completion_resolve(&self, client: ClientId, msg: &Value) -> bool {
        let orig_id = msg["id"].as_i64().unwrap_or(0);
        let null = Value::Null;
        let params = msg.get("params").unwrap_or(&null);
        let key = completion_item_key(params);

        let mut owners: HashSet<usize> = HashSet::new();
        if let Some(key) = key.as_ref() {
            let provenance = self
                .inner
                .completion_provenance
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for uri_entries in provenance.values() {
                if let Some((child_index, _)) = uri_entries.get(key) {
                    owners.insert(*child_index);
                }
            }
        }
        let owner = match owners.len() {
            0 => None,
            1 => owners.iter().next().copied(),
            _ => {
                // Deux enfants distincts revendiquent la même `(label,kind)` :
                // on ne devine pas — le faux enrichissement (ou l'erreur du
                // mauvais serveur) coûte plus cher que le no-op.
                tracing::warn!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    "LSP: completionItem/resolve provenance ambiguous across children, no-op fallback"
                );
                None
            }
        };
        let Some(child_index) = owner else {
            if owners.is_empty() {
                tracing::debug!(
                    toolchain = self.inner._toolchain_name.as_str(),
                    "LSP: completionItem/resolve without provenance, no-op fallback (item served as-is)"
                );
            }
            self.send_resolve_fallback(client, orig_id, params);
            return true;
        };

        if child_index == PRIMARY_PART {
            // Provenance primaire : le chemin historique de `send` est
            // exactement ce routage (session id + `pending` + réponse id
            // restauré par la lectrice primaire).
            return false;
        }

        let Some(aux) = self.inner.aux.get(child_index - 1) else {
            // Index fantôme (barrière d'une autre vie — ne peut pas arriver
            // tant que `aux` est figé au spawn ; défense = fallback).
            tracing::warn!(
                toolchain = self.inner._toolchain_name.as_str(),
                child_index,
                "LSP: completionItem/resolve provenance points to unknown child, no-op fallback"
            );
            self.send_resolve_fallback(client, orig_id, params);
            return true;
        };
        if !aux.alive.load(Ordering::SeqCst) {
            // Enfant déjà mort à l'aiguillage : AUCUNE trame sur un fil mort.
            tracing::warn!(
                toolchain = self.inner._toolchain_name.as_str(),
                role = aux.role.as_str(),
                "LSP: completionItem/resolve owner already degraded, no-op fallback"
            );
            self.send_resolve_fallback(client, orig_id, params);
            return true;
        }

        let internal_id = aux.next_internal_req.fetch_add(1, Ordering::SeqCst);
        let mut copy = msg.clone();
        copy["id"] = Value::Number(serde_json::Number::from(internal_id));
        aux.pending_internal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                internal_id,
                InternalPending::ClientResolve {
                    client,
                    orig_id,
                    item: params.clone(),
                },
            );
        // Re-check `alive` APRÈS `try_send` (même raison que le fan-out
        // task-07) : un aux mort juste après l'enregistrement aurait purgé
        // sans voir notre entrée — sans ce re-check le client pendrait.
        let unreachable = aux
            .stdin_tx
            .try_send(copy.to_string().into_bytes())
            .is_err()
            || !aux.alive.load(Ordering::SeqCst);
        if unreachable {
            aux.pending_internal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&internal_id);
            tracing::warn!(
                toolchain = self.inner._toolchain_name.as_str(),
                role = aux.role.as_str(),
                "LSP: completionItem/resolve cannot reach owner, no-op fallback"
            );
            self.send_resolve_fallback(client, orig_id, params);
        }
        true
    }

    /// Tâche 08 — fallback no-op d'un `completionItem/resolve` : réponse au
    /// client avec l'item TEL QUEL (aucun enrichissement), sous l'id d'origine
    /// — JAMAIS une erreur globale (contrat : un resolve non servi est
    /// inoffensif, un resolve en erreur perturbe l'éditeur).
    fn send_resolve_fallback(&self, client: ClientId, orig_id: i64, item: &Value) {
        self.send_to_client(
            client,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": orig_id,
                "result": item.clone(),
            }),
        );
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
    /// de ce client, ses barrières de fusion `merges` en cours (tâche 07,
    /// règle 7 — aucune réponse ne sera jamais émise vers un client désabonné,
    /// et les `ClientMerge` d'aux déjà enregistrés se font avaler à l'arrivée :
    /// entrée de barrière absente ⟹ debug) et sa présence dans toutes les sets
    /// de `editor_uris` (déconnexion = plus de tenant, piste R1 sq1 — sans ça,
    /// une URI fermée brutalement (WS coupé, pas de `didClose`) resterait
    /// tenue pour toujours).
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
        // Retirer les barrières de fusion de ce client (tâche 07, règle 7).
        self.inner
            .merges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, pending_merge| pending_merge.client != client);
        // Retirer les resolve completion en attente de ce client (tâche 08) :
        // aucune réponse ne partira vers un client désabonné (règle 7) ; la
        // réponse tardive de l'aux sur cet id interne purgé sera avalée comme
        // inconnue par la lectrice (warn existant), sans résidu observable.
        for aux in &self.inner.aux {
            aux.pending_internal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, entry| {
                    !matches!(
                        entry,
                        InternalPending::ClientResolve { client: c, .. } if *c == client
                    )
                });
        }
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
    /// Multiplexeur : requête avec `id` → primaire par défaut, CHEMIN
    /// INCHANGÉ ; en composite (`aux` non vide) les méthodes
    /// `MergeRoute::All` (`merge_route_for_method`) sont de plus fan-out vers
    /// chaque aux vivant sous id interne propre avec barrière `PendingMerge`
    /// (tâche 07 — une seule réponse client fusionnée à la fin). `initialize`
    /// garde sa copie systématique vers chaque aux vivant
    /// (`fan_out_initialize`, invariable 3). `completionItem/resolve` passe
    /// D'ABORD par sa troisième voie dédiée en composite
    /// (`route_completion_resolve`, tâche 08 — routage par provenance, jamais
    /// de barrière ni de primaire aveugle). Trame sans `id` (notification,
    /// doc-sync inclus) → primaire puis fan-out telle quelle à chaque aux
    /// vivant (invariant 2) — un aux mort ou bloqué est ignoré, ne bloque ni
    /// n'erre jamais le chemin client. `aux` vide ⟹ la table de routage n'est
    /// JAMAIS consultée, chemin mono-process strictement actuel (invariant 8).
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
            // ── Tâche 08 — `completionItem/resolve` : troisième voie dédiée
            // en composite (ni barrière, ni primaire aveugle), AVANT toute
            // écriture d'id. `aux` vide ⟹ la table de provenance n'est PAS
            // consultée, chemin primaire historique strict (invariant 8 — les
            // serveurs mono ont leur propre mémoire d'items).
            if !self.inner.aux.is_empty()
                && msg.get("method").and_then(|m| m.as_str()) == Some("completionItem/resolve")
                && self.route_completion_resolve(client, &msg)
            {
                return Ok(());
            }

            // Requête : réécrire l'id en session id et mémoriser le mapping
            let session_req_id = self.inner.next_req.fetch_add(1, Ordering::SeqCst);
            let orig_id = msg["id"].as_i64().unwrap_or(0);
            let method = msg.get("method").and_then(|m| m.as_str());

            // Réécrire l'id dans le payload pour le processus
            let mut rewritten = msg.clone();
            rewritten["id"] = Value::Number(serde_json::Number::from(session_req_id));
            let rewritten_payload = rewritten.to_string().into_bytes();

            // Mémoriser le mapping session_id -> (client, orig_id)
            if let Ok(mut pending) = self.inner.pending.lock() {
                pending.insert(session_req_id, (client, orig_id));
            }

            // ── Tâche 07, règle 1 — barrière composite pour les méthodes
            // `Route::All`. `aux` vide ⟹ la table n'est PAS consultée
            // (invariant 8). La barrière est insérée AVANT l'envoi au
            // primaire : le primaire pourrait répondre avant qu'elle existe,
            // et sa réponse partirait alors par le chemin historique — deux
            // réponses sortantes. Le primaire garde son ids-space historique
            // (`next_req`/`pending` ci-dessus, inchangés) ; les aux recevront
            // des ids internes corrélés par `ClientMerge`.
            let barrier = !self.inner.aux.is_empty()
                && method.is_some_and(|m| merge_route_for_method(m) == MergeRoute::All);
            let fan_targets: Vec<Arc<AuxChild>> = if barrier {
                let alive: Vec<Arc<AuxChild>> = self
                    .inner
                    .aux
                    .iter()
                    .filter(|aux| aux.alive.load(Ordering::SeqCst))
                    .map(Arc::clone)
                    .collect();
                self.inner
                    .merges
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        session_req_id,
                        PendingMerge {
                            client,
                            orig_id,
                            method: method.unwrap_or_default().to_string(),
                            // URI de la requête pour la provenance completion
                            // (tâche 08) — extraite pour toutes les méthodes
                            // `All`, consommée par completion seule.
                            params_uri: msg
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            expected: alive.len() + 1,
                            parts: HashMap::new(),
                        },
                    );
                alive
            } else {
                Vec::new()
            };

            // Envoyer au process
            let sent = self.inner.cmd_tx.send(rewritten_payload).await;
            if sent.is_err() && barrier {
                // Le primaire est mort avant de recevoir la requête : aucune
                // part 0 n'arrivera jamais, la barrière est retirée (le
                // chemin historique ne se purge pas ici — inchangé, il fuitait
                // déjà son `pending` sur cette erreur avant la tâche 07).
                self.inner
                    .merges
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&session_req_id);
            }
            sent.map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"))?;

            if barrier {
                // Fan-out vers chaque aux VIVANT compté à l'insertion de la
                // barrière : même payload, id interne de l'enfant,
                // `ClientMerge { session_id }` enregistré AVANT l'envoi (pas
                // de course avec la lectrice aux). `try_send` + re-check
                // `alive` : un aux injoignable ne compte pas dans `expected`
                // (règle 1 — jamais d'attente perpétuelle) et ne fait jamais
                // échouer le chemin client.
                for aux in &fan_targets {
                    let unreachable = !aux.alive.load(Ordering::SeqCst);
                    if unreachable {
                        self.abandon_merge_expectation(session_req_id, aux.role.as_str());
                        continue;
                    }
                    let internal_id = aux.next_internal_req.fetch_add(1, Ordering::SeqCst);
                    let mut copy = msg.clone();
                    copy["id"] = Value::Number(serde_json::Number::from(internal_id));
                    aux.pending_internal
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(
                            internal_id,
                            InternalPending::ClientMerge {
                                session_id: session_req_id,
                            },
                        );
                    // Re-check `alive` APRÈS `try_send` : un aux mort entre
                    // le comptage et l'envoi aurait pu purger ses ids internes
                    // AVANT notre enregistrement — sa part n'arriverait
                    // jamais et pendrait. Injoignable ⟹ même traitement que
                    // `try_send` en échec.
                    let unreachable = aux
                        .stdin_tx
                        .try_send(copy.to_string().into_bytes())
                        .is_err()
                        || !aux.alive.load(Ordering::SeqCst);
                    if unreachable {
                        aux.pending_internal
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&internal_id);
                        self.abandon_merge_expectation(session_req_id, aux.role.as_str());
                    }
                }
                // Un aux exclus ici était peut-être la seule part manquante
                // (primaire + autres aux déjà consignés) : tenter la
                // complétion, sans quoi la fusion ne partirait jamais.
                self.try_complete_merge(session_req_id);
            }

            // Invariant 3 : `initialize` part AUSSI en copie (id interne propre,
            // `initOptions` du spec injectés) vers chaque aux vivant. Les autres
            // méthodes `MergeRoute::Primary` restent primaire uniquement —
            // chemin actuel exact (règle 8).
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
            // Tâche 08 — un document qui change ou se ferme rend suspects les
            // items servis de sa dernière complétion : purge immédiate de sa
            // provenance (un resolve qui retombe alors en no-op est l'échec
            // acceptable du cache). Mono-process : jamais écrit, jamais purgé
            // — ci-dessus le `return` du chemin historique (invariant 8).
            if matches!(
                msg.get("method").and_then(|m| m.as_str()),
                Some("textDocument/didChange") | Some("textDocument/didClose")
            ) && let Some(uri) = msg
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            {
                self.inner
                    .completion_provenance
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(uri);
            }
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

    /// Nombre de barrières de fusion actuellement ouvertes (test tâche 07 :
    /// garde de l'invariant 8 — zéro en mono-process — et vérification que la
    /// barrière est bien retirée après complétion, erreur primaire ou
    /// unsubscribe). Jamais utilisé hors tests.
    #[cfg(test)]
    pub(crate) fn pending_merge_count(&self) -> usize {
        self.inner
            .merges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Nombre d'URI tenues par le cache de provenance completion (test tâche
    /// 08 : garde mono-process — jamais écrit ; vérification composite — écrit
    /// à la complétion, purgé au didChange/didClose). Jamais utilisé hors
    /// tests.
    #[cfg(test)]
    pub(crate) fn completion_provenance_uri_count(&self) -> usize {
        self.inner
            .completion_provenance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
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

    /// Diagnostics en cache pour `uri` — le FONDU multi-enfants (tâche 06) :
    /// concat des parts publiées par chaque enfant dans l'ordre croissant de
    /// leur index — primaire (0) d'abord, puis aux dans l'ordre de `inner.aux`
    /// — indépendant de l'ordre d'arrivée des publications. Mono-process
    /// (`aux: []`), contenu exactement publié par le primaire (inchangé).
    /// `None` si jamais publiés (ou invalidés) pour cette URI.
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

    /// Retire l'entrée `uri` du `diagnostics_cache` ET de `diag_parts` (toutes
    /// parts d'enfant). Indispensable avant une ré-analyse :
    /// `wait_for_diagnostics` retourne le cache s'il est présent — sans
    /// invalidation, edit_and_check verrait le stale d'AVANT l'édition
    /// (design §7 étape 2). La purge des parts est du même coup : sans elle,
    /// une part aux stale survivrait masquée dans `diag_parts` et resurgirait
    /// dans le fondu à la première publication suivante. Aucun notify, aucun
    /// effet si absent.
    pub fn invalidate_diagnostics(&self, uri: &str) {
        self.inner
            .diagnostics_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(uri);
        self.inner
            .diag_parts
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

    /// Test 23: composite_other_request_primary_only — une requête (id) dont la
    /// méthode est hors table `MergeRoute::All` reste intégralement sur le
    /// primaire : réponse au client, l'aux ne la voit jamais. Sonde
    /// `textDocument/signatureHelp` (méthode hors table All, défaut `Primary`) —
    /// le `hover` d'origine est devenu une méthode fan-out par la tâche 07.
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

        // initialize + didOpen AVANT la sonde : prouvent que le canal de l'aux
        // est vivant (l'absence de la sonde ensuite est significative, pas
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
            "l'aux doit avoir reçu le didOpen (canal vivant) avant l'assertion signatureHelp; trames: {aux_frames:?}"
        );

        let signature_help = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/signatureHelp",
            "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}}
        });
        session
            .send(client, signature_help.to_string().into_bytes())
            .await
            .expect("signatureHelp send ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit répondre de signatureHelp");
        assert_eq!(
            serde_json::from_slice::<Value>(&resp).unwrap()["id"].as_i64(),
            Some(2)
        );

        let saw_signature_help = |frames: &[Value]| {
            frames.iter().any(|f| {
                f.get("method").and_then(|m| m.as_str()) == Some("textDocument/signatureHelp")
            })
        };
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            saw_signature_help,
            Duration::from_secs(5),
        )
        .await;
        assert!(
            saw_signature_help(&primary_frames),
            "le primaire doit recevoir la sonde signatureHelp; trames: {primary_frames:?}"
        );

        // Fenêtre laissée à l'aux pour la recevoir éventuellement : rien.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !saw_signature_help(&aux_frames),
            "l'aux ne doit PAS recevoir de requête hors table All (sonde signatureHelp); trames: {aux_frames:?}"
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

    // ── Tests tâche 06 (vue-lsp) : fusion publishDiagnostics avant cache ─────

    /// Script Python factice PRIMAIRE « publieur de diagnostics » (tâche 06) :
    /// à chaque `textDocument/didOpen`, publie une `publishDiagnostics` sur
    /// l'URI ouverte portant un unique diagnostic `{"source":"primary",
    /// "message":"P{n}"}` (n = compteur de publications — deux `didOpen` ⟹ P1
    /// puis P2, motif REPUBLISH étendu avec `source` distinctif). Journalise
    /// dans argv[1] (optionnel) ses trames PUBLIÉES en octets bruts (une par
    /// ligne) — base de la comparaison octet-à-octet du test mono-process.
    /// Répond `{"result":{"echo":…}}` à toute requête. Modes via `fake_mode` :
    /// `"slow"` (dort 400 ms avant chaque publication — laisser une part
    /// d'abord arriver de l'autre enfant) ; `"manual"` (ne publie PAS sur
    /// didOpen — retient la dernière URI ouverte et publie sur la notification
    /// `x/trigger-publish` : au didOpen seul l'aux publie, au trigger les deux
    /// publient).
    const FAKE_LSP_PRIMARY_DIAGS_PY: &str = r#"
import sys, json, time

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

pub_count = 0
last_uri = ""

def publish():
    global pub_count
    pub_count += 1
    notif = {
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": last_uri,
            "diagnostics": [{"source": "primary", "message": f"P{pub_count}"}]
        }
    }
    out = json.dumps(notif).encode("utf-8")
    if LOG:
        with open(LOG, "ab") as f:
            f.write(out + b"\n")
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
    method = msg.get("method", "")
    if method == "textDocument/didOpen":
        last_uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
        if MODE == "manual":
            continue
        if MODE == "slow":
            time.sleep(0.4)
        publish()
    elif method == "x/trigger-publish":
        if MODE == "manual":
            last_uri = msg.get("params", {}).get("textDocument", {}).get("uri", "") or last_uri
            if last_uri:
                publish()
    elif "id" in msg:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}})
"#;

    /// Script Python factice AUX « publieur de diagnostics » (tâche 06) : publie
    /// sur l'URI du `textDocument/didOpen` un unique diagnostic
    /// `{"source":"aux","message":"A1"}` ; répond `{"result":{}}` à toute
    /// requête (copie d'`initialize`). Journalise ses publications en octets
    /// bruts dans argv[1] (optionnel). Modes via `fake_mode` : `"once"` (ne
    /// publie qu'AU PREMIER didOpen, reste vivant — les publications suivantes
    /// sont primaire-only) ; `"slow"` (dort 400 ms avant publication — arrive
    /// après le primaire) ; `"trigger"` (publie aussi sur `x/trigger-publish`) ;
    /// `"die"` (publie au premier didOpen puis MEURT À LA TRAME SUIVANTE : le
    /// test envoie une notification `fake/kill` après avoir observé le fondu —
    /// la mort est déclenchée par le test, pas une course qui rendrait l'état
    /// fondu transitoire fugace par construction du fake).
    const FAKE_LSP_AUX_DIAGS_PY: &str = r#"
import sys, json, time

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

did_open_count = 0

def publish(uri):
    notif = {
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "diagnostics": [{"source": "aux", "message": "A1"}]
        }
    }
    out = json.dumps(notif).encode("utf-8")
    if LOG:
        with open(LOG, "ab") as f:
            f.write(out + b"\n")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()
    if MODE == "die":
        # meurt à la trame SUIVANTE (le test envoie `fake/kill` après avoir
        # observé le fondu [P1, A1]) — mort déclenchée par le test, pas une
        # course qui ferait disparaître l'état fondu avant qu'on l'observe.
        read_frame()
        sys.exit(0)

while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    method = msg.get("method", "")
    if method == "textDocument/didOpen":
        did_open_count += 1
        if MODE == "once" and did_open_count > 1:
            continue
        uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
        if MODE == "slow":
            time.sleep(0.4)
        publish(uri)
    elif method == "x/trigger-publish" and MODE == "trigger":
        uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
        if uri:
            publish(uri)
    elif "id" in msg:
        out = json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {}}).encode("utf-8")
        sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
        sys.stdout.buffer.write(out)
        sys.stdout.buffer.flush()
"#;

    /// Diagnostic attendu côté primaire dans un fondu.
    fn prim_diag(message: &str) -> Value {
        serde_json::json!({"source": "primary", "message": message})
    }

    /// Diagnostic attendu côté aux dans un fondu.
    fn aux_diag(message: &str) -> Value {
        serde_json::json!({"source": "aux", "message": message})
    }

    /// Poll `cached_diagnostics` (10 ms, borné par `timeout`) jusqu'à `done` —
    /// motif de `poll_recorder` appliqué au cache : les parts arrivent de deux
    /// process indépendants, seul l'état FONDU final est déterministe.
    async fn poll_cached(
        session: &LspSession,
        uri: &str,
        done: impl Fn(&Option<Vec<Value>>) -> bool,
        timeout: Duration,
    ) -> Option<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let cached = session.cached_diagnostics(uri);
            if done(&cached) || tokio::time::Instant::now() >= deadline {
                return cached;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Prédicat `poll_cached` : le cache est EXACTEMENT `expected` (fondu
    /// complet — equality stricte, pas de part surnuméraire ni de stale).
    fn cache_is(expected: Vec<Value>) -> impl Fn(&Option<Vec<Value>>) -> bool {
        move |cached| cached.as_ref() == Some(&expected)
    }

    /// Vide le flux client jusqu'à 100 ms d'inactivité : trames
    /// `publishDiagnostics` reçues (octets bruts + parse), dans l'ordre reçu.
    async fn drain_publish_frames(
        rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> Vec<(Vec<u8>, Value)> {
        let mut pubs = Vec::new();
        while let Some(raw) = recv_timeout(rx, Duration::from_millis(100)).await {
            if let Ok(v) = serde_json::from_slice::<Value>(&raw)
                && v.get("method").and_then(|m| m.as_str())
                    == Some("textDocument/publishDiagnostics")
            {
                pubs.push((raw, v));
            }
        }
        pubs
    }

    /// Les frames des fakes (python `json.dumps`) sérialisent avec le
    /// séparateur `": "` ; le fondu synthétisé par le multiplexeur (serde
    /// compact) ne le contient jamais : l'absence de ce motif prouve que le
    /// client n'a reçu AUCUNE part brute d'un seul enfant, uniquement des
    /// publications fondues (contrat tâche 06, mode composite).
    fn is_synthesized(raw: &[u8]) -> bool {
        !raw.windows(3).any(|w| w == b"\": \"")
    }

    /// Test 35: diag_merge_primary_then_aux_order — didOpen unique, primaire
    /// publié d'abord (aux `slow`) : `cached_diagnostics` = `[P1, A1]` et le
    /// flux client porte le MÊME fondu en dernière notification (aucune part
    /// brute d'un seul enfant sur le réseau — tout est synthétisé).
    #[tokio::test]
    async fn diag_merge_primary_then_aux_order() {
        let aux_script = fake_mode(FAKE_LSP_AUX_DIAGS_PY, "slow");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_PRIMARY_DIAGS_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();
        let uri = "file:///w/App.vue";
        composite_open(&session, client, uri).await;

        let merged = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            merged,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "fondu primary-first dans le cache (aux arrivé en second)"
        );

        let pubs = drain_publish_frames(&mut rx).await;
        assert!(
            !pubs.is_empty(),
            "le client doit recevoir les publications fondues"
        );
        let (raw_last, last) = pubs.last().unwrap();
        assert_eq!(last["params"]["uri"].as_str(), Some(uri));
        assert_eq!(
            last["params"]["diagnostics"],
            serde_json::json!([prim_diag("P1"), aux_diag("A1")]),
            "dernier broadcast = le fondu complet [P1, A1], jamais une part seule; flux: {pubs:?}"
        );
        for (raw, _) in &pubs {
            assert!(
                is_synthesized(raw),
                "part brute d'un seul enfant broadcastée (octets du fake) : {}",
                String::from_utf8_lossy(raw)
            );
        }
        assert!(
            is_synthesized(raw_last),
            "la dernière frame doit être synthétisée"
        );

        drop(tmpdir);
    }

    /// Test 36: diag_merge_order_independent_of_arrival — l'AUX publie AVANT
    /// le primaire (primaire `slow`) : le fondu final reste `[P1, A1]` — ordre
    /// par index d'enfant, pas par ordre d'arrivée (et pas last-writer-wins :
    /// le flux fini n'est pas `[A1]`).
    #[tokio::test]
    async fn diag_merge_order_independent_of_arrival() {
        let primary_script = fake_mode(FAKE_LSP_PRIMARY_DIAGS_PY, "slow");
        let (spec, tmpdir) = make_fake_composite(
            primary_script.as_str(),
            &[("tsserver-forward", FAKE_LSP_AUX_DIAGS_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();
        let uri = "file:///w/App.vue";
        composite_open(&session, client, uri).await;

        let merged = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            merged,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "aux publié en premier : le fondu reste primary-first par index d'enfant"
        );

        let pubs = drain_publish_frames(&mut rx).await;
        let (_raw, last) = pubs.last().expect("le flux porte les publications fondues");
        assert_eq!(
            last["params"]["diagnostics"],
            serde_json::json!([prim_diag("P1"), aux_diag("A1")]),
            "flux fini = fondu complet [P1, A1], pas [A1] (aux arrivé en premier) ; flux: {pubs:?}"
        );

        drop(tmpdir);
    }

    /// Test 37: diag_replace_per_child_not_append — le primaire republie
    /// (2e didOpen ⟹ P2) : le fondu devient `[P2, A1]` — P1 nulle part, ni
    /// dans le cache ni dans le dernier broadcast. La publication d'un enfant
    /// REMPLACE sa part, ne l'ajoute jamais.
    #[tokio::test]
    async fn diag_replace_per_child_not_append() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_PRIMARY_DIAGS_PY,
            &[("tsserver-forward", FAKE_LSP_AUX_DIAGS_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();
        let uri = "file:///w/App.vue";

        composite_open(&session, client, uri).await;
        let first = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            first,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "état de départ [P1, A1]"
        );

        // 2e didOpen : primaire republie P2 (remplacement de SA part), aux
        // republie A1 (remplacement idempotent de la sienne).
        composite_open(&session, client, uri).await;
        let merged = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P2"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            merged,
            Some(vec![prim_diag("P2"), aux_diag("A1")]),
            "P1 nulle part dans le cache : la part du primaire est remplacée, pas appendue"
        );

        let pubs = drain_publish_frames(&mut rx).await;
        let (_raw, last) = pubs.last().expect("le flux porte les publications fondues");
        assert_eq!(
            last["params"]["diagnostics"],
            serde_json::json!([prim_diag("P2"), aux_diag("A1")]),
            "dernier broadcast fondu sans P1 non plus ; flux: {pubs:?}"
        );

        drop(tmpdir);
    }

    /// Test 38: diag_invalidate_clears_all_parts — après `[P2, A1]`,
    /// `invalidate_diagnostics` vide le cache ET les parts : la publication
    /// primaire suivante (aux `once` silencieux) produit le fondu `[P3]`
    /// EXACTEMENT — sans invalidation des parts, le stale aux survivrait
    /// masqué et resurgirait en `[P3, A1]`.
    #[tokio::test]
    async fn diag_invalidate_clears_all_parts() {
        let aux_script = fake_mode(FAKE_LSP_AUX_DIAGS_PY, "once");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_PRIMARY_DIAGS_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, _rx) = session.subscribe();
        let uri = "file:///w/App.vue";

        composite_open(&session, client, uri).await;
        let first = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            first,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "état de départ [P1, A1]"
        );

        // 2e didOpen (aux `once` ne republie plus) → [P2, A1] : la part aux
        // vit toujours, rien ne l'a invalidée.
        composite_open(&session, client, uri).await;
        let second = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P2"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            second,
            Some(vec![prim_diag("P2"), aux_diag("A1")]),
            "la part aux survit tant que rien ne l'invalidise"
        );

        session.invalidate_diagnostics(uri);
        assert!(
            session.cached_diagnostics(uri).is_none(),
            "l'invalidation retire l'entrée du cache"
        );

        // 3e didOpen → primaire seul publie P3 : fondu EXACTEMENT [P3] — la
        // part aux stale est partie avec l'invalidation.
        composite_open(&session, client, uri).await;
        let fresh = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P3")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            fresh,
            Some(vec![prim_diag("P3")]),
            "une publication primaire seule après invalidation ne ressuscite aucune part aux"
        );

        drop(tmpdir);
    }

    /// Test 39: diag_wait_resolves_on_aux_push — `wait_for_diagnostics` est
    /// réveillé par la publication de l'AUX seul (primaire `manual` muet au
    /// didOpen) et rend `[A1]` : le MCP ne dépend jamais du primaire pour être
    /// réveillé. Invalidation puis trigger (les DEUX enfants republient) : le
    /// cache repassé en attente rend le fondu complet `[P1, A1]`.
    #[tokio::test]
    async fn diag_wait_resolves_on_aux_push() {
        let primary_script = fake_mode(FAKE_LSP_PRIMARY_DIAGS_PY, "manual");
        let aux_script = fake_mode(FAKE_LSP_AUX_DIAGS_PY, "trigger");
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
        let uri = "file:///w/App.vue";

        composite_open(&session, client, uri).await; // primaire manual : muet au didOpen

        // Le MCP est réveillé par l'AUX seul — si le réveil dépendait du
        // primaire, ce wait ne sortirait jamais avant son timeout.
        let aux_only = session
            .wait_for_diagnostics(uri, Duration::from_secs(5))
            .await;
        assert_eq!(
            aux_only,
            Some(vec![aux_diag("A1")]),
            "la publication de l'aux seule doit réveiller wait_for_diagnostics"
        );

        // Invalidation (cache ET parts) puis trigger : le fan-out de la
        // notification (invariant 2) fait republier les DEUX enfants.
        session.invalidate_diagnostics(uri);
        assert!(
            session.cached_diagnostics(uri).is_none(),
            "invalidé = plus rien en attente"
        );
        let trigger = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "x/trigger-publish",
            "params": {"textDocument": {"uri": uri}}
        });
        session
            .send(client, trigger.to_string().into_bytes())
            .await
            .expect("trigger ok");

        let merged = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            merged,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "les deux enfants ayant republié, le fondu est complet (primary-first)"
        );
        let waited = session
            .wait_for_diagnostics(uri, Duration::from_secs(1))
            .await;
        assert_eq!(
            waited,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "un wait repassé en attente rend le fondu complet"
        );

        let pubs = drain_publish_frames(&mut rx).await;
        let (_raw, last) = pubs.last().expect("le flux porte les publications fondues");
        assert_eq!(
            last["params"]["diagnostics"],
            serde_json::json!([prim_diag("P1"), aux_diag("A1")]),
            "dernière publication fondue du flux = [P1, A1] ; flux: {pubs:?}"
        );

        drop(tmpdir);
    }

    /// Test 40: diag_aux_eof_drops_its_parts — aux publie A1, primaire publie
    /// P1 → `[P1, A1]` ; puis l'aux meurt (mode `die`, tué par une notification
    /// `fake/kill` que le fake attend après sa publication) : ses parts sont
    /// retirées et le cache recompté en `[P1]`, URI toujours présente (le cache
    /// primaire vivant reste publieux) et session vivante. Sans purge, un
    /// `edit_and_check` passerait au vert sur les erreurs fantômes d'un serveur
    /// mort.
    #[tokio::test]
    async fn diag_aux_eof_drops_its_parts() {
        let aux_script = fake_mode(FAKE_LSP_AUX_DIAGS_PY, "die");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_PRIMARY_DIAGS_PY,
            &[("tsserver-forward", aux_script.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, _rx) = session.subscribe();
        let uri = "file:///w/App.vue";

        composite_open(&session, client, uri).await;
        let both = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1"), aux_diag("A1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            both,
            Some(vec![prim_diag("P1"), aux_diag("A1")]),
            "les deux parts sont publiées avant la mort de l'aux"
        );

        // L'aux en mode `die` meurt à la trame suivante — le trigger passe par
        // le fan-out des notifications (invariant 2) ; le primaire l'ignore.
        let kill = serde_json::json!({"jsonrpc": "2.0", "method": "fake/kill", "params": {}});
        session
            .send(client, kill.to_string().into_bytes())
            .await
            .expect("fake/kill ok");

        // EOF de l'aux : ses parts partent, le cache recompte [P1] — et lui
        // seul. Atteignable seulement après l'EOF (la part A1 vue ci-dessus
        // n'est retirée que par la purge d'EOF).
        let after = poll_cached(
            &session,
            uri,
            cache_is(vec![prim_diag("P1")]),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            after,
            Some(vec![prim_diag("P1")]),
            "EOF aux ⟹ ses parts retirées et le cache recompté, URI toujours présente"
        );
        assert!(
            session.is_alive(),
            "la session survit à la mort de l'aux et son cache primaire reste servi"
        );

        drop(tmpdir);
    }

    /// Test 41: diag_mono_process_bytes_unchanged — `aux: []` : le payload
    /// broadcast reçu par le client est OCTET-IDENTIQUE à la trame publiée
    /// par le fake primaire (aucune synthèse JSON réordonnée — garde de
    /// l'invariant 8 sur CE chemin précis) ; le cache vaut le contenu publié.
    #[tokio::test]
    async fn diag_mono_process_bytes_unchanged() {
        // Mono-process construit à la main : le fake journalise ses
        // publications en octets bruts (argv[1]) pour la comparaison exacte.
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join("primary_diags.py");
        std::fs::write(&script_path, FAKE_LSP_PRIMARY_DIAGS_PY).unwrap();
        let published_log = tmpdir.path().join("published.log");
        let spec = LspToolchain {
            name: "mono".to_string(),
            bin: "python3".to_string(),
            args: vec![
                script_path.to_string_lossy().to_string(),
                published_log.to_string_lossy().to_string(),
            ],
            aux: vec![],
        };
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn mono-process ok");
        let (client, mut rx) = session.subscribe();
        let uri = "file:///w/main.rs";
        composite_open(&session, client, uri).await;

        let raw = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le client reçoit la publication");
        // Le fake écrit son log AVANT stdout : la trame reçue ⟹ le log est
        // déjà écrit. Ligne = payload brut + `\n` de terminaison.
        let logged = std::fs::read(&published_log).expect("le fake a journalisé sa publication");
        let expected = logged
            .strip_suffix(b"\n" as &[u8])
            .expect("log terminé par \\n")
            .to_vec();
        assert_eq!(
            raw, expected,
            "broadcast mono-process = octets EXACTS de la trame publiée par le fake (pas de synthèse)"
        );
        assert_eq!(
            session.cached_diagnostics(uri),
            Some(vec![prim_diag("P1")]),
            "cache mono-process = contenu publié, inchangé"
        );
        assert!(session.is_alive());

        drop(tmpdir);
    }

    // ── Tests tâche 07 (vue-lsp) : routage par méthode + barrière de fusion ─

    /// Script Python factice PRIMAIRE « fusions de requêtes » (tâche 07) :
    /// journalise les trames reçues dans argv[1] (pattern recorder de la tâche
    /// 03) et répond des résultats FIXES identifiables par méthode LSP (le
    /// `hover` echo la ligne de position — distinguo des requêtes
    /// simultanées). `completion` répond un `CompletionList` fixture
    /// `{"isIncomplete":false,"items":[tmpl(14), ref(17,"P")]}` — variante
    /// SANS `ref` sur une URI en `B.vue` (fabrique l'ambiguïté du test
    /// `resolve_ambiguous_no_wire`). `completionItem/resolve` renvoie l'item
    /// reçu ENRICHI `_from:"prim"` (tâche 08) et fait partie de la famille
    /// « méthodes de fusion » : les modes ci-dessous qui la dégradent
    /// (`null`/`error`/`slow`/`die`/`hold`/`swap`) s'appliquent aussi à lui.
    /// Toute requête hors famille (dont `initialize`) répond
    /// `{"capabilities":{}}` — les modes ne dégradent JAMAIS la séquence
    /// d'initialisation. Modes via `fake_mode` :
    /// `"null"` (toute la famille répond `result: null`) ; `"error"`
    /// (toute la famille répond une erreur JSON-RPC -32001) ; `"slow"`
    /// (400 ms de sleep avant chaque réponse) ; `"die"` (meurt APRÈS avoir
    /// reçu une trame de la famille, sans répondre — purge d'EOF task-05) ;
    /// `"single"` (`definition` répondu comme `Location` objet seul, pas en
    /// tableau) ; `"docchanges"` (`rename` répondu en variante
    /// `documentChanges` — cas mixte avec le primaire `changes`) ; `"swap"`
    /// (retient les requêtes, répond en ordre inversé à la 2e — corrélation) ;
    /// `"hold"` (retient les `textDocument/*` jusqu'à la notification
    /// `x/release`, puis redevient normal — determinisme des tests de mort de
    /// barrière) ; `"completion-array"` (completion répond le même jeu d'items
    /// en TABLEAU simple, pas en `CompletionList` — shape de sortie) ;
    /// `"completion-kind"` (completion répond `[{"label":"x","kind":17}]` —
    /// dédup kind absent ≠ kind présent).
    const FAKE_LSP_MERGE_PRIMARY_PY: &str = r#"
import sys, json, time

LOG = sys.argv[1] if len(sys.argv) > 1 else ""
MODE = "@@MODE@@"
SIDE = "primary"
FROM = "prim"

R = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}

def position_line(msg):
    return msg.get("params", {}).get("position", {}).get("line", 0)

def completion_items(msg):
    # Items fixture du primaire (tâche 08). URI en B.vue : sans `ref` ⟹
    # (ref,17) retenu depuis l'AUX sur cette URI, depuis le PRIMAIRE sur les
    # autres — même clé provenance, deux enfants distincts (ambiguïté).
    uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
    if "B.vue" in uri:
        return [{"label": "tmpl", "kind": 14}]
    return [{"label": "tmpl", "kind": 14}, {"label": "ref", "kind": 17, "detail": "P"}]

def completion(msg):
    return {"isIncomplete": False, "items": completion_items(msg)}

RESULTS = {
    "textDocument/hover": lambda m: {"contents": {"kind": "markdown", "value": f"H-PRIM:{position_line(m)}"}},
    "textDocument/definition": lambda m: [{"uri": "file:///a", "range": R}],
    "textDocument/typeDefinition": lambda m: [{"uri": "file:///a-td", "range": R}],
    "textDocument/implementation": lambda m: [{"uri": "file:///a-impl", "range": R}],
    "textDocument/references": lambda m: [{"uri": "file:///a-ref", "range": R}],
    "textDocument/codeAction": lambda m: [{"title": "ca-prim"}],
    "textDocument/prepareRename": lambda m: {"range": R, "placeholder": "prim"},
    "textDocument/rename": lambda m: {"changes": {
        "file:///a": [{"range": R, "newText": "prim"}],
        "file:///shared": [{"range": R, "newText": "shared-prim"}]}},
    "textDocument/completion": completion,
}

SINGLE = {
    "textDocument/definition": lambda m: {"uri": "file:///a", "range": R},
}

DOCCHANGES = {
    "textDocument/rename": lambda m: {"documentChanges": [
        {"textDocument": {"uri": "file:///a"}, "edits": [{"range": R, "newText": "dc-prim"}]}]},
}

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

def log(msg):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(msg) + "\n")

def compute(msg):
    method = msg.get("method", "")
    if MODE == "null":
        return None
    if method == "completionItem/resolve":
        params = msg.get("params")
        item = dict(params) if isinstance(params, dict) else {}
        item["_from"] = FROM
        return item
    if MODE == "completion-array" and method == "textDocument/completion":
        return completion_items(msg)
    if MODE == "completion-kind" and method == "textDocument/completion":
        return [{"label": "x", "kind": 17}]
    if MODE == "single" and method in SINGLE:
        return SINGLE[method](msg)
    if MODE == "docchanges" and method in DOCCHANGES:
        return DOCCHANGES[method](msg)
    return RESULTS[method](msg) if method in RESULTS else {"echo": method}

def answer(msg):
    return {"jsonrpc": "2.0", "id": msg["id"], "result": compute(msg)}

hold_buffer = []
released = False

while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    log(msg)
    if "id" not in msg:
        if msg.get("method") == "x/release" and MODE == "hold":
            for m in hold_buffer:
                write_frame(answer(m))
            hold_buffer = []
            released = True
        continue
    method = msg.get("method", "")
    in_family = method.startswith("textDocument/") or method == "completionItem/resolve"
    if not in_family:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
        continue
    if MODE == "die":
        sys.exit(0)
    if MODE == "error":
        write_frame({"jsonrpc": "2.0", "id": msg["id"],
                     "error": {"code": -32001, "message": f"{SIDE} exploded on {method}"}})
        continue
    if MODE == "hold" and not released and method.startswith("textDocument/"):
        hold_buffer.append(msg)
        continue
    if MODE == "swap":
        hold_buffer.append(msg)
        if len(hold_buffer) < 2:
            continue
        for m in reversed(hold_buffer):
            write_frame(answer(m))
        hold_buffer = []
        continue
    if MODE == "slow":
        time.sleep(0.4)
    write_frame(answer(msg))
"#;

    /// Script Python factice AUX « fusions de requêtes » (tâche 07) : miroir
    /// exact du primaire avec des résultats distinguables (`H-AUX:<ligne>`,
    /// URIs `file:///b*`, `ca-aux`, placeholder `aux`, edits `*-aux`) et la
    /// même famille de modes (`null`/`error`/`slow`/`die`/`single`/
    /// `docchanges`/`swap`/`hold`). L'aux d'un composite de fusion porte le
    /// rôle spawnable `tsserver-forward` — la fusion ne dépend pas du rôle.
    /// Tâche 08 : `completion` répond le fixture en TABLEAU
    /// `[script(3), ref(17,"A")]`, `completionItem/resolve` renvoie l'item reçu
    /// enrichi `_from:"aux"`, et le resolve fait partie de la famille dégradée
    /// par les modes. Modes supplémentaires (résolution task-08) :
    /// `"completion-list"` (completion en `CompletionList{isIncomplete:true}`
    /// — shape + OR d'incomplétude) ; `"completion-nokind"` (completion
    /// `[{"label":"x"}]` sans kind — dédup kind absent ≠ présent) ;
    /// `"error-resolve"` (complétion normale, erreur JSON-RPC sur les
    /// `completionItem/resolve` seulement — fallback item tel quel) ;
    /// `"hold-resolve"` (complétion normale, retient les resolve jusqu'à
    /// `x/release` — determinisme des tests de purge) ; `"hold-resolve-die"`
    /// (retient les resolve puis MEURT sur la notification `fake/kill` — EOF
    /// avec resolve en attente).
    const FAKE_LSP_MERGE_AUX_PY: &str = r#"
import sys, json, time

LOG = sys.argv[1] if len(sys.argv) > 1 else ""
MODE = "@@MODE@@"
SIDE = "aux"
FROM = "aux"

R = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}

def position_line(msg):
    return msg.get("params", {}).get("position", {}).get("line", 0)

AUX_ITEMS = [{"label": "script", "kind": 3}, {"label": "ref", "kind": 17, "detail": "A"}]

def completion(msg):
    return list(AUX_ITEMS)

RESULTS = {
    "textDocument/hover": lambda m: {"contents": {"kind": "markdown", "value": f"H-AUX:{position_line(m)}"}},
    "textDocument/definition": lambda m: [{"uri": "file:///b", "range": R}],
    "textDocument/typeDefinition": lambda m: [{"uri": "file:///b-td", "range": R}],
    "textDocument/implementation": lambda m: [{"uri": "file:///b-impl", "range": R}],
    "textDocument/references": lambda m: [{"uri": "file:///b-ref", "range": R}],
    "textDocument/codeAction": lambda m: [{"title": "ca-aux"}],
    "textDocument/prepareRename": lambda m: {"range": R, "placeholder": "aux"},
    "textDocument/rename": lambda m: {"changes": {
        "file:///b": [{"range": R, "newText": "aux"}],
        "file:///shared": [{"range": R, "newText": "shared-aux"}]}},
    "textDocument/completion": completion,
}

SINGLE = {
    "textDocument/definition": lambda m: {"uri": "file:///b", "range": R},
}

DOCCHANGES = {
    "textDocument/rename": lambda m: {"documentChanges": [
        {"textDocument": {"uri": "file:///b"}, "edits": [{"range": R, "newText": "dc-aux"}]}]},
}

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

def log(msg):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(msg) + "\n")

def compute(msg):
    method = msg.get("method", "")
    if MODE == "null":
        return None
    if method == "completionItem/resolve":
        params = msg.get("params")
        item = dict(params) if isinstance(params, dict) else {}
        item["_from"] = FROM
        return item
    if MODE == "completion-list" and method == "textDocument/completion":
        return {"isIncomplete": True, "items": list(AUX_ITEMS)}
    if MODE == "completion-nokind" and method == "textDocument/completion":
        return [{"label": "x"}]
    if MODE == "single" and method in SINGLE:
        return SINGLE[method](msg)
    if MODE == "docchanges" and method in DOCCHANGES:
        return DOCCHANGES[method](msg)
    return RESULTS[method](msg) if method in RESULTS else {"echo": method}

def answer(msg):
    return {"jsonrpc": "2.0", "id": msg["id"], "result": compute(msg)}

hold_buffer = []
released = False

while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    log(msg)
    if "id" not in msg:
        if msg.get("method") == "x/release" and MODE in ("hold", "hold-resolve"):
            for m in hold_buffer:
                write_frame(answer(m))
            hold_buffer = []
            released = True
        if msg.get("method") == "fake/kill" and MODE == "hold-resolve-die":
            sys.exit(0)
        continue
    method = msg.get("method", "")
    in_family = method.startswith("textDocument/") or method == "completionItem/resolve"
    if not in_family:
        write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
        continue
    if MODE == "die":
        sys.exit(0)
    if MODE == "error" or (MODE == "error-resolve" and method == "completionItem/resolve"):
        write_frame({"jsonrpc": "2.0", "id": msg["id"],
                     "error": {"code": -32001, "message": f"{SIDE} exploded on {method}"}})
        continue
    if MODE == "hold" and not released and method.startswith("textDocument/"):
        hold_buffer.append(msg)
        continue
    if MODE in ("hold-resolve", "hold-resolve-die") and method == "completionItem/resolve":
        hold_buffer.append(msg)
        continue
    if MODE == "swap":
        hold_buffer.append(msg)
        if len(hold_buffer) < 2:
            continue
        for m in reversed(hold_buffer):
            write_frame(answer(m))
        hold_buffer = []
        continue
    if MODE == "slow":
        time.sleep(0.4)
    write_frame(answer(msg))
"#;

    /// Composite de deux fakes de fusion (primaire + aux `tsserver-forward` —
    /// seul rôle spawnable, la fusion n'en dépend pas), chacun dans son mode
    /// (`""` = défaut). Le tmpdir est retenu pour la durée du test.
    async fn make_merge_composite(
        primary_mode: &str,
        aux_mode: &str,
    ) -> (Arc<LspSession>, tempfile::TempDir) {
        let primary = fake_mode(FAKE_LSP_MERGE_PRIMARY_PY, primary_mode);
        let aux = fake_mode(FAKE_LSP_MERGE_AUX_PY, aux_mode);
        let (spec, tmpdir) = make_fake_composite(
            primary.as_str(),
            &[("tsserver-forward", aux.as_str(), vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite de fusion ok");
        (session, tmpdir)
    }

    /// Requête `textDocument/*` pour les fakes de fusion (position à la ligne
    /// `line` — echoée dans les `hover`, distinguo des requêtes simultanées).
    fn text_request(id: i64, method: &str, uri: &str, line: i64) -> Vec<u8> {
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method,
            "params": {
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": 0}
            }
        })
        .to_string()
        .into_bytes()
    }

    /// Parse une trame reçue par un client.
    fn client_msg(raw: &[u8]) -> Value {
        serde_json::from_slice(raw).expect("trame client JSON valide")
    }

    /// Range fixe porté par tous les résultats des fakes de fusion.
    fn merge_range() -> Value {
        serde_json::json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}})
    }

    /// `Location` attendu d'un côté de fusion (`side` = "a"/"b", `suffix` =
    /// ""/"-td"/"-impl"/"-ref").
    fn merge_location(side: &str, suffix: &str) -> Value {
        serde_json::json!({"uri": format!("file:///{side}{suffix}"), "range": merge_range()})
    }

    /// `hover` primaire seul (partie unique non-null ⟹ restitué verbatim).
    fn prim_hover(line: i64) -> Value {
        serde_json::json!({"contents": {"kind": "markdown", "value": format!("H-PRIM:{line}")}})
    }

    /// `hover` fusionné markup primary-first, séparateur `---`.
    fn merged_hover(line: i64) -> Value {
        serde_json::json!({"contents": {"kind": "markdown", "value": format!("H-PRIM:{line}\n\n---\n\nH-AUX:{line}")}})
    }

    /// `WorkspaceEdit` du primaire (`changes` : URI propre + URI partagée).
    fn prim_workspace_edit() -> Value {
        serde_json::json!({"changes": {
            "file:///a": [{"range": merge_range(), "newText": "prim"}],
            "file:///shared": [{"range": merge_range(), "newText": "shared-prim"}]
        }})
    }

    /// `WorkspaceEdit` attendu de la fusion `changes` : edits de l'URI
    /// partagée concaténés primary-first dans le MÊME tableau, URI disjointe
    /// de l'aux (`file:///b`) ajoutée. Les parts brutes des fakes
    /// (`prim_workspace_edit`, et côté aux `file:///b` + `shared-aux`) ne
    /// sortent jamais telles quelles côté client.
    fn merged_workspace_edit() -> Value {
        serde_json::json!({"changes": {
            "file:///a": [{"range": merge_range(), "newText": "prim"}],
            "file:///shared": [
                {"range": merge_range(), "newText": "shared-prim"},
                {"range": merge_range(), "newText": "shared-aux"}
            ],
            "file:///b": [{"range": merge_range(), "newText": "aux"}]
        }})
    }

    /// Prédicat recorder : trame requête de cette méthode. `Copy` (capture
    /// `&'static str`) — utilisable dans plusieurs closures sans déplacement.
    fn is_method(method: &'static str) -> impl Fn(&Value) -> bool + Copy {
        move |f: &Value| f.get("method").and_then(|m| m.as_str()) == Some(method)
    }

    /// Test 42: merge_hover_concat_markup — un `hover` composite produit UNE
    /// réponse client (id d'origine restauré), markup concaténé primary-first
    /// avec séparateur `---` et kind du primaire ; primaire et aux ont chacun
    /// reçu le hover dans LEUR espace d'ids (session id 3 pour le primaire —
    /// initialize 1, ping 2, hover 3 ; id interne 2 pour l'aux — seul le
    /// copie d'initialize 1 l'a précédé ; le ping a désynchronisé les
    /// compteurs), l'id client 7 n'apparaît sur aucun fil.
    #[tokio::test]
    async fn merge_hover_concat_markup() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;

        // ping : avance seul le compteur de session du primaire — les deux
        // espaces d'ids divergent à partir d'ici.
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":5,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("ping ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("réponse ping du primaire");
        assert_eq!(client_msg(&resp)["id"].as_i64(), Some(5));

        session
            .send(
                client,
                text_request(7, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse hover unique"),
        );
        assert_eq!(
            resp["id"].as_i64(),
            Some(7),
            "id d'origine restauré sur la réponse fusionnée"
        );
        assert_eq!(
            resp["result"],
            merged_hover(0),
            "markup fondu primary-first, séparateur ---, kind du primaire"
        );

        let is_hover = is_method("textDocument/hover");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_hover),
            Duration::from_secs(5),
        )
        .await;
        let prim = primary_frames
            .iter()
            .find(|f| is_hover(f))
            .expect("le primaire doit recevoir le hover");
        assert_eq!(
            prim["id"].as_i64(),
            Some(3),
            "fil primaire : session id (espace historique réécrit), jamais l'id client; trames: {primary_frames:?}"
        );

        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_hover),
            Duration::from_secs(5),
        )
        .await;
        let aux = aux_frames
            .iter()
            .find(|f| is_hover(f))
            .expect("l'aux doit recevoir le hover (fan-out Route::All)");
        assert_eq!(
            aux["id"].as_i64(),
            Some(2),
            "fil aux : id interne (espace propre à l'enfant), jamais l'id client; trames: {aux_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "une seule réponse client par requête fusionnée: {leaked:?}"
        );
        assert_eq!(
            session.pending_merge_count(),
            0,
            "barrière retirée après complétion"
        );

        drop(tmpdir);
    }

    /// Test 43: merge_locations_definition_concat — `definition` : `[LocA]` +
    /// `[LocB]` ⟹ `[LocA, LocB]` primary-first ; `references` avec un seul
    /// côté non-null ⟹ CE côté, jamais de tableau vide parasite.
    #[tokio::test]
    async fn merge_locations_definition_concat() {
        // Les deux répondent ⟹ concaténation primary-first.
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/definition", "file:///w/App.vue", 0),
            )
            .await
            .expect("definition ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse definition"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert_eq!(
            resp["result"],
            serde_json::json!([merge_location("a", ""), merge_location("b", "")]),
            "locations concaténées primary-first"
        );
        drop(tmpdir);

        // Aux null ⟹ le tableau du primaire seul, sans tableau vide parasite.
        let (session, tmpdir) = make_merge_composite("", "null").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/references", "file:///w/App.vue", 0),
            )
            .await
            .expect("references ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse references"),
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([merge_location("a", "-ref")]),
            "un seul côté non-null ⟹ ce côté"
        );
        drop(tmpdir);

        // Primaire null ⟹ le tableau de l'aux.
        let (session, tmpdir) = make_merge_composite("null", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/references", "file:///w/App.vue", 0),
            )
            .await
            .expect("references ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse references"),
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([merge_location("b", "-ref")]),
            "primaire null ⟹ la part non-null de l'aux"
        );

        drop(tmpdir);
    }

    /// Test 44: merge_location_single_object_wrapped — le primaire répond un
    /// `Location` objet SEUL (pas en tableau), l'aux `null` ⟹ `[objet]` côté
    /// client (wrap en tableau de la forme objet unique).
    #[tokio::test]
    async fn merge_location_single_object_wrapped() {
        let (session, tmpdir) = make_merge_composite("single", "null").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/definition", "file:///w/App.vue", 0),
            )
            .await
            .expect("definition ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse definition"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert_eq!(
            resp["result"],
            serde_json::json!([merge_location("a", "")]),
            "Location objet seul wrapé en tableau"
        );

        drop(tmpdir);
    }

    /// Test 45: merge_codeaction_concat_null_is_empty — primaire `[CA1]` +
    /// aux `null` ⟹ `[CA1]` ; primaire `null` + aux ⟹ la part de l'aux ;
    /// les deux répondent ⟹ `[CA1, CA2]` (ordre primary-first respecté quand
    /// les deux répondent — les codeActions des deux fakes portent des titres
    /// distincts pour rendre l'ordre observable).
    #[tokio::test]
    async fn merge_codeaction_concat_null_is_empty() {
        // Les deux répondent ⟹ concat primary-first.
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/codeAction", "file:///w/App.vue", 0),
            )
            .await
            .expect("codeAction ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse codeAction"),
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([{"title": "ca-prim"}, {"title": "ca-aux"}]),
            "les deux répondent : ordre primary-first"
        );
        drop(tmpdir);

        // Aux null ⟹ le tableau du primaire seul (null n'est jamais `[]` parasite).
        let (session, tmpdir) = make_merge_composite("", "null").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/codeAction", "file:///w/App.vue", 0),
            )
            .await
            .expect("codeAction ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse codeAction"),
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([{"title": "ca-prim"}]),
            "aux null ⟹ contribution vide, jamais un élément parasite"
        );
        drop(tmpdir);

        // Primaire null ⟹ la part de l'aux (fusion ⟹ tableau, jamais null).
        let (session, tmpdir) = make_merge_composite("null", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/codeAction", "file:///w/App.vue", 0),
            )
            .await
            .expect("codeAction ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse codeAction"),
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([{"title": "ca-aux"}]),
            "primaire null ⟹ codeActions de l'aux (null ⟹ tableau, pas null)"
        );

        drop(tmpdir);
    }

    /// Test 46: merge_prepare_rename_primary_wins_then_fallback — les deux
    /// non-null ⟹ résultat du PRIMAIRE verbatim ; primaire null avec aux
    /// `{range}` ⟹ résultat de l'aux (fallback).
    #[tokio::test]
    async fn merge_prepare_rename_primary_wins_then_fallback() {
        let prim_result = serde_json::json!({"range": merge_range(), "placeholder": "prim"});
        let aux_result = serde_json::json!({"range": merge_range(), "placeholder": "aux"});

        // Les deux non-null ⟹ le primaire gagne.
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/prepareRename", "file:///w/App.vue", 0),
            )
            .await
            .expect("prepareRename ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse prepareRename"),
        );
        assert_eq!(
            resp["result"], prim_result,
            "les deux non-null ⟹ résultat primaire verbatim"
        );
        drop(tmpdir);

        // Primaire null ⟹ premier non-null aux.
        let (session, tmpdir) = make_merge_composite("null", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/prepareRename", "file:///w/App.vue", 0),
            )
            .await
            .expect("prepareRename ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse prepareRename"),
        );
        assert_eq!(
            resp["result"], aux_result,
            "primaire null ⟹ fallback sur le premier non-null aux"
        );

        drop(tmpdir);
    }

    /// Test 46b: merge_rename_changes_concat — `changes` sur URI commune ⟹
    /// edits concaténés primary-first dans le MÊME tableau, URI disjointe de
    /// l'aux ajoutée ; variantes mixtes (primaire `changes`, aux
    /// `documentChanges`) ⟹ variante du PRIMAIRE conservée intégralement,
    /// edits de l'aux abandonnés (`tracing::warn!` explicite dans
    /// `fold_workspace_edit` — vérifié par inspection du code, pas
    /// observable ici).
    #[tokio::test]
    async fn merge_rename_changes_concat() {
        // Les deux en `changes` ⟹ merge objet URI par URI, primary-first.
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/rename", "file:///w/App.vue", 0),
            )
            .await
            .expect("rename ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse rename"),
        );
        assert_eq!(
            resp["result"],
            merged_workspace_edit(),
            "URI commune : edits concaténés primary-first dans le même tableau ; URI disjointe ajoutée"
        );
        drop(tmpdir);

        // Mixte : primaire `changes`, aux `documentChanges` ⟹ variante du
        // primaire conservée telle quelle.
        let (session, tmpdir) = make_merge_composite("", "docchanges").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/rename", "file:///w/App.vue", 0),
            )
            .await
            .expect("rename ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse rename"),
        );
        assert_eq!(
            resp["result"],
            prim_workspace_edit(),
            "variante mixte ⟹ variante primaire conservée, edits aux abandonnés"
        );

        drop(tmpdir);
    }

    /// Test 47: merge_aux_error_part_skipped — erreur de l'aux sur hover ⟹
    /// réponse client = hover du primaire seul (part None), PAS une erreur
    /// globale.
    #[tokio::test]
    async fn merge_aux_error_part_skipped() {
        let (session, tmpdir) = make_merge_composite("", "error").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse hover avec part aux vide"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert!(
            resp.get("error").is_none(),
            "l'erreur d'un aux ne devient JAMAIS l'erreur de la requête: {resp}"
        );
        assert_eq!(
            resp["result"],
            prim_hover(0),
            "part aux None ⟹ hover primaire seul (une seule contribution non-null)"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "une seule réponse: {leaked:?}");
        assert_eq!(session.pending_merge_count(), 0);

        drop(tmpdir);
    }

    /// Test 48: merge_primary_error_fails_fast — erreur du PRIMAIRE ⟹ réponse
    /// erreur IMMÉDIATE au client (id restauré, structure `error` verbatim),
    /// barrière tuée ; l'aux (vivant, répond ensuite) ne produit RIEN de
    /// visible — aucune 2e réponse ; session saine.
    #[tokio::test]
    async fn merge_primary_error_fails_fast() {
        let (session, tmpdir) = make_merge_composite("error", "").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse erreur immédiate du primaire"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2), "id d'origine restauré");
        assert_eq!(
            resp["error"],
            serde_json::json!({"code": -32001, "message": "primary exploded on textDocument/hover"}),
            "structure error du primaire transmise TELLE QUELLE"
        );
        assert!(resp.get("result").is_none());
        assert_eq!(
            session.pending_merge_count(),
            0,
            "la barrière est tuée par l'erreur primaire"
        );

        // L'aux (mode normal) répond son hover — part tardive avalée sans
        // bruit par la barrière disparue : aucune 2e réponse.
        let leaked = recv_timeout(&mut rx, Duration::from_millis(600)).await;
        assert!(
            leaked.is_none(),
            "les parts aux tardives après erreur primaire sont avalées: {leaked:?}"
        );

        // Session saine.
        session
            .send(
                client,
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{}})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .expect("ping ok");
        let resp = recv_timeout(&mut rx, Duration::from_secs(5))
            .await
            .expect("le primaire doit rester fonctionnel après son erreur");
        assert_eq!(client_msg(&resp)["id"].as_i64(), Some(3));

        drop(tmpdir);
    }

    /// Test 49: merge_aux_eof_mid_merge — l'aux meurt (mode `die`) APRÈS avoir
    /// reçu le fan-out, avant de répondre ⟹ sa part `ClientMerge` en attente
    /// est levée en `None` à l'EOF (purge task-05 étendue) ⟹ barrière
    /// complétée, réponse primaire seule, client servi UNE fois.
    #[tokio::test]
    async fn merge_aux_eof_mid_merge() {
        let (session, tmpdir) = make_merge_composite("", "die").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("la mort de l'aux en pleine fusion ne doit pas pendre"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert_eq!(
            resp["result"],
            prim_hover(0),
            "EOF aux ⟹ part None ⟹ hover primaire seul, la fusion continue"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "une seule réponse: {leaked:?}");
        assert!(
            session.is_alive(),
            "la session survit à l'EOF de l'aux en pleine fusion"
        );
        assert_eq!(
            session.pending_merge_count(),
            0,
            "barrière complétée et retirée, rien ne pend"
        );

        drop(tmpdir);
    }

    /// Test 50: merge_concurrent_no_cross — deux `hover` simultanés (ids
    /// clients 1, 2 ; lignes de position 0 et 7 echoées dans les résultats),
    /// l'aux répond EN ORDRE INVERSÉ (mode `swap`) ⟹ chaque client reçoit SA
    /// fusion correcte (les payloads distincts par position ne se croisent
    /// jamais).
    #[tokio::test]
    async fn merge_concurrent_no_cross() {
        let (session, tmpdir) = make_merge_composite("", "swap").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;

        session
            .send(
                client,
                text_request(1, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover 1 ok");
        session
            .send(
                client,
                text_request(2, "textDocument/hover", "file:///w/Other.vue", 7),
            )
            .await
            .expect("hover 2 ok");

        // L'aux répond au 2e hover AVANT le 1er (swap) : l'ordre d'arrivée
        // des réponses client est imprévisible — corrélation par id, pas par
        // ordre.
        let mut results_by_id = HashMap::new();
        for _ in 0..2 {
            let raw = recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("les deux fusions doivent parvenir au client");
            let msg = client_msg(&raw);
            results_by_id.insert(msg["id"].as_i64().unwrap(), msg["result"].clone());
        }
        assert_eq!(
            results_by_id[&1],
            merged_hover(0),
            "la fusion du hover id 1 (ligne 0) ne doit rien croiser avec le hover id 2"
        );
        assert_eq!(
            results_by_id[&2],
            merged_hover(7),
            "la fusion du hover id 2 (ligne 7) doit garder ses deux moitiés appariées"
        );
        assert_eq!(session.pending_merge_count(), 0);

        drop(tmpdir);
    }

    /// Test 51: merge_unsubscribe_mid_merge — souscrire → `hover` → `
    /// unsubscribe` → l'aux libère alors sa réponse retenue (mode `hold`,
    /// déclenchement par `x/release` : aucune course de timing) ⟹ AUCUNE
    /// réponse émise vers le client parti, pas de panique, barrière purgée —
    /// observable : le hover suivant d'un nouveau client se fuse normalement.
    #[tokio::test]
    async fn merge_unsubscribe_mid_merge() {
        let (session, tmpdir) = make_merge_composite("", "hold").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;

        session
            .send(
                client,
                text_request(2, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover ok");
        // Le primaire a répondu (part 0 consignée) ; l'aux retient la sienne.
        tokio::time::sleep(Duration::from_millis(150)).await;
        session.unsubscribe(client);
        assert_eq!(
            session.pending_merge_count(),
            0,
            "unsubscribe purge les barrières du client"
        );

        // L'aux libère sa réponse : la barrière est partie ⟹ part avalée en
        // silence (debug), JAMAIS de réponse vers un client désabonné.
        let release = serde_json::json!({"jsonrpc": "2.0", "method": "x/release", "params": {}});
        session
            .send(client, release.to_string().into_bytes())
            .await
            .expect("x/release ok");
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "aucune réponse vers un client désabonné: {leaked:?}"
        );

        // Barrière purgée = le mécanisme est vierge : un hover suivant complet.
        let (client2, mut rx2) = session.subscribe();
        session
            .send(
                client2,
                text_request(3, "textDocument/hover", "file:///w/App.vue", 0),
            )
            .await
            .expect("hover 2 ok");
        let resp = client_msg(
            &recv_timeout(&mut rx2, Duration::from_secs(5))
                .await
                .expect("le hover suivant doit se fuser normalement"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3));
        assert_eq!(resp["result"], merged_hover(0));
        assert_eq!(session.pending_merge_count(), 0);

        drop(tmpdir);
    }

    // ── Tests tâche 08 (vue-lsp) : fusion completion + resolve par provenance ─

    /// `CompletionList` du primaire en mode défaut (fixture task-08) — aussi la
    /// réponse VERBATIM attendue en mono-process (chemin historique, aucune
    /// fusion possible).
    fn prim_completion_list() -> Value {
        serde_json::json!({"isIncomplete": false, "items": [
            {"label": "tmpl", "kind": 14},
            {"label": "ref", "kind": 17, "detail": "P"}
        ]})
    }

    /// Résultat fusionné attendu des modes défaut (test 1 et préambules) :
    /// `[tmpl, ref(P), script]` — le `ref(17,"A")` de l'aux est droppé par le
    /// dédup `(label,kind)` primary-wins ; shape `CompletionList` (le primaire
    /// en a envoyé un), `isIncomplete` false.
    fn merged_completion_fixture() -> Value {
        serde_json::json!({"isIncomplete": false, "items": [
            {"label": "tmpl", "kind": 14},
            {"label": "ref", "kind": 17, "detail": "P"},
            {"label": "script", "kind": 3}
        ]})
    }

    /// Item attendu d'un fake en mode resolve : les `params` reçus enrichis
    /// `_from` (contrat des fakes task-08).
    fn resolved(item: Value, from: &str) -> Value {
        let mut enriched = item.as_object().unwrap().clone();
        enriched.insert("_from".to_string(), Value::String(from.to_string()));
        Value::Object(enriched)
    }

    /// Préambule commun task-08 : `initialize` (id client 1) puis
    /// `textDocument/completion` (id client 2) fusionnée sur `uri` ; rend la
    /// réponse client unique. Les modes des fakes sont ceux du composite déjà
    /// spawné par l'appelant.
    async fn completion_setup(
        session: &LspSession,
        client: ClientId,
        rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
        uri: &str,
    ) -> Value {
        composite_initialize(session, client, rx).await;
        session
            .send(client, text_request(2, "textDocument/completion", uri, 0))
            .await
            .expect("completion ok");
        let resp = client_msg(
            &recv_timeout(rx, Duration::from_secs(5))
                .await
                .expect("réponse completion fusionnée"),
        );
        assert_eq!(
            resp["id"].as_i64(),
            Some(2),
            "id client restauré sur la fusion"
        );
        resp
    }

    /// Requête `completionItem/resolve` : `params` = l'item lui-même (les
    /// éditeurs renvoient l'item servi tel quel).
    fn resolve_request(id: i64, item: Value) -> Vec<u8> {
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "completionItem/resolve",
            "params": item
        })
        .to_string()
        .into_bytes()
    }

    /// Notification `textDocument/didChange` pleine (seule l'URI importe pour
    /// la purge de provenance, mais la shape doit être honnête).
    fn did_change(uri: &str) -> Vec<u8> {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {"textDocument": {"uri": uri}, "contentChanges": [{"text": "<template/>"}]}
        })
        .to_string()
        .into_bytes()
    }

    /// Tâche 08 test 1 : merge_completion_replaces_stays_primary_guard —
    /// (remplace `merge_completion_stays_primary`, garde d'AVANT cette tâche) :
    /// la completion fan-out est reçue par les DEUX enfants (session id 2 pour
    /// le primaire — initialize=1, completion=2 ; id interne 2 pour l'aux —
    /// copie d'initialize=1), réponse client UNIQUE fusionnée
    /// `[tmpl, ref(P), script]` — le `ref` de l'aux (detail `A`) est droppé par
    /// le dédup `(label,kind)`, le detail `P` du primaire survit (primary-
    /// wins) ; shape `CompletionList`, `isIncomplete` false ; provenance
    /// écrite pour l'URI.
    #[tokio::test]
    async fn merge_completion_replaces_stays_primary_guard() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        let resp = completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;
        assert_eq!(
            resp["result"],
            merged_completion_fixture(),
            "items fusionnés primary-first, dédup (label,kind) primary-wins, shape CompletionList, isIncomplete false"
        );
        assert_eq!(
            session.pending_merge_count(),
            0,
            "barrière retirée après complétion"
        );
        assert_eq!(
            session.completion_provenance_uri_count(),
            1,
            "provenance écrite pour l'URI complétée"
        );

        let is_completion = is_method("textDocument/completion");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_completion),
            Duration::from_secs(5),
        )
        .await;
        let prim = primary_frames
            .iter()
            .find(|f| is_completion(f))
            .expect("le primaire reçoit la completion");
        assert_eq!(
            prim["id"].as_i64(),
            Some(2),
            "fil primaire : session id (initialize=1, completion=2) ; trames: {primary_frames:?}"
        );
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_completion),
            Duration::from_secs(5),
        )
        .await;
        let aux = aux_frames.iter().find(|f| is_completion(f)).expect(
            "l'AUX reçoit la completion (Route::All — l'inverse exact de la garde task-07)",
        );
        assert_eq!(
            aux["id"].as_i64(),
            Some(2),
            "fil aux : id interne (copie initialize=1, completion=2) ; trames: {aux_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse client unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 2 : merge_completion_incomplete_or_and_array_shape —
    /// primaire en TABLEAU simple, aux en `CompletionList{isIncomplete:true}`
    /// ⟹ sortie `CompletionList` (au moins une part en était un), `isIncomplete`
    /// = OR des parts = true, items concaténés-dédupés primary-first.
    #[tokio::test]
    async fn merge_completion_incomplete_or_and_array_shape() {
        let (session, tmpdir) = make_merge_composite("completion-array", "completion-list").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/completion", "file:///w/App.vue", 0),
            )
            .await
            .expect("completion ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse completion"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert_eq!(
            resp["result"],
            serde_json::json!({"isIncomplete": true, "items": [
                {"label": "tmpl", "kind": 14},
                {"label": "ref", "kind": 17, "detail": "P"},
                {"label": "script", "kind": 3}
            ]}),
            "une part CompletionList ⟹ shape CompletionList ; isIncomplete OR(true) ; items dédupés primary-first"
        );
        drop(tmpdir);
    }

    /// Tâche 08 test 3 : merge_completion_kind_absent_distinct — aux
    /// `{"label":"x"}` sans kind vs primaire `{"label":"x","kind":17}` ⟹ les
    /// DEUX servis (kind absent ≠ kind présent dans la clé de dédup) ; aucune
    /// part `CompletionList` ⟹ shape tableau.
    #[tokio::test]
    async fn merge_completion_kind_absent_distinct() {
        let (session, tmpdir) = make_merge_composite("completion-kind", "completion-nokind").await;
        let (client, mut rx) = session.subscribe();
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(2, "textDocument/completion", "file:///w/App.vue", 0),
            )
            .await
            .expect("completion ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse completion"),
        );
        assert_eq!(resp["id"].as_i64(), Some(2));
        assert_eq!(
            resp["result"],
            serde_json::json!([{"label": "x", "kind": 17}, {"label": "x"}]),
            "kind absent ≠ kind présent ⟹ les DEUX servis ; toutes parts tableaux ⟹ shape tableau"
        );
        drop(tmpdir);
    }

    /// Tâche 08 test 4 : resolve_routes_to_aux — après la fixture : resolve de
    /// `script(3)` (item servi par l'AUX) ⟹ l'aux reçoit la requête sous son
    /// id interne 3 (initialize=1, completion=2, resolve=3), le primaire NON,
    /// réponse client = résultat de l'AUX avec l'id d'origine, réponse unique.
    #[tokio::test]
    async fn resolve_routes_to_aux() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        let item = serde_json::json!({"label": "script", "kind": 3});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("résultat du resolve par l'aux"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3), "id d'origine restauré");
        assert_eq!(
            resp["result"],
            resolved(item.clone(), "aux"),
            "résultat de L'AUX (item enriché _from=aux) — jamais une réponse du primaire"
        );

        let is_resolve = is_method("completionItem/resolve");
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_resolve),
            Duration::from_secs(5),
        )
        .await;
        let aux = aux_frames
            .iter()
            .find(|f| is_resolve(f))
            .expect("l'AUX reçoit le resolve (provenance : item retenu depuis lui)");
        assert_eq!(
            aux["id"].as_i64(),
            Some(3),
            "fil aux : id interne (initialize=1, completion=2, resolve=3) ; trames: {aux_frames:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        let primary_frames = read_recorder_frames(&primary_log_path(&tmpdir));
        assert!(
            !primary_frames.iter().any(is_resolve),
            "le primaire ne reçoit PAS le resolve d'un item de l'aux ; trames: {primary_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 5 : resolve_routes_to_primary — resolve de `tmpl(14)`
    /// (item servi par le PRIMAIRE) ⟹ primaire seul sur le fil (session id 3,
    /// chemin historique intact), jamais l'aux, réponse = résultat primaire
    /// enrichi avec l'id d'origine, réponse unique.
    #[tokio::test]
    async fn resolve_routes_to_primary() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        let item = serde_json::json!({"label": "tmpl", "kind": 14});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("résultat du resolve par le primaire"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3), "id d'origine restauré");
        assert_eq!(
            resp["result"],
            resolved(item.clone(), "prim"),
            "résultat du PRIMAIRE (item enriché _from=prim)"
        );

        let is_resolve = is_method("completionItem/resolve");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_resolve),
            Duration::from_secs(5),
        )
        .await;
        let prim = primary_frames
            .iter()
            .find(|f| is_resolve(f))
            .expect("le primaire reçoit le resolve de SON item");
        assert_eq!(
            prim["id"].as_i64(),
            Some(3),
            "fil primaire : session id historique (initialize=1, completion=2, resolve=3) ; trames: {primary_frames:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !aux_frames.iter().any(is_resolve),
            "l'aux ne reçoit PAS le resolve d'un item du primaire ; trames: {aux_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 6 : resolve_unknown_item_no_wire — resolve d'un item
    /// jamais servi (`inconnu(1)`) ⟹ réponse immédiate = item TEL QUEL (pas de
    /// `_from`, pas une erreur), AUCUNE trame resolve sur les deux fils.
    #[tokio::test]
    async fn resolve_unknown_item_no_wire() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        let item = serde_json::json!({"label": "inconnu", "kind": 1});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("fallback immédiat item tel quel"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3));
        assert!(
            resp.get("error").is_none(),
            "jamais une erreur globale: {resp}"
        );
        assert_eq!(
            resp["result"], item,
            "item renvoyé TEL QUEL (resolve no-op, sans enrichissement)"
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        let is_resolve = is_method("completionItem/resolve");
        let primary_frames = read_recorder_frames(&primary_log_path(&tmpdir));
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !primary_frames.iter().any(is_resolve) && !aux_frames.iter().any(is_resolve),
            "AUCUNE trame resolve sur les deux fils en fallback: prim={primary_frames:?} aux={aux_frames:?}"
        );
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 7 : resolve_didChange_purges_provenance — completion,
    /// puis `didChange` de l'URI, puis resolve du `script` ⟹ fallback item tel
    /// quel (la purge a rendu la provenance introuvable), AUCUN fil.
    // Nom de test imposé verbatim par la tâche 08 (`didChange` = nom LSP de la
    // notification) — d'où l'allow ciblé plutôt qu'un renommage.
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn resolve_didChange_purges_provenance() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        let uri = "file:///w/App.vue";
        completion_setup(&session, client, &mut rx, uri).await;
        assert_eq!(
            session.completion_provenance_uri_count(),
            1,
            "état de départ : provenance de l'URI présente"
        );

        session
            .send(client, did_change(uri))
            .await
            .expect("didChange ok");
        assert_eq!(
            session.completion_provenance_uri_count(),
            0,
            "didChange purge la provenance de CETTE URI"
        );

        let item = serde_json::json!({"label": "script", "kind": 3});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("fallback item tel quel après purge"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3));
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"], item, "item TEL QUEL, aucun enrichissement");

        tokio::time::sleep(Duration::from_millis(300)).await;
        let is_resolve = is_method("completionItem/resolve");
        let primary_frames = read_recorder_frames(&primary_log_path(&tmpdir));
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !primary_frames.iter().any(is_resolve) && !aux_frames.iter().any(is_resolve),
            "aucun fil après purge: prim={primary_frames:?} aux={aux_frames:?}"
        );
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 8 : resolve_ambiguous_no_wire — deux completions sur deux
    /// URI amenant le MÊME `(label,kind)` (`ref`,17) depuis deux enfants
    /// DISTINCTS (l'aux le sert partout ; le primaire ne le sert que hors
    /// `B.vue` — fixture.URI-aware du fake) ⟹ resolve ambigü ⟹ fallback item
    /// tel quel, AUCUN fil.
    #[tokio::test]
    async fn resolve_ambiguous_no_wire() {
        let (session, tmpdir) = make_merge_composite("", "").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        // 2e completion sur l'URI B : le primaire n'y renvoie PAS `ref` ⟹
        // (ref,17) retenu depuis l'aux ICI, depuis le primaire sur App.
        session
            .send(
                client,
                text_request(3, "textDocument/completion", "file:///w/B.vue", 0),
            )
            .await
            .expect("completion B ok");
        let resp_b = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse completion B fusionnée"),
        );
        assert_eq!(resp_b["id"].as_i64(), Some(3));
        assert_eq!(
            session.completion_provenance_uri_count(),
            2,
            "les deux URI portent leur provenance"
        );

        let item = serde_json::json!({"label": "ref", "kind": 17});
        session
            .send(client, resolve_request(4, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("fallback immédiat ambigu"),
        );
        assert_eq!(resp["id"].as_i64(), Some(4));
        assert!(
            resp.get("error").is_none(),
            "jamais une erreur globale: {resp}"
        );
        assert_eq!(
            resp["result"], item,
            "item TEL QUEL (ambigu ⟹ on ne devine pas)"
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        let is_resolve = is_method("completionItem/resolve");
        let primary_frames = read_recorder_frames(&primary_log_path(&tmpdir));
        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            !primary_frames.iter().any(is_resolve) && !aux_frames.iter().any(is_resolve),
            "resolve ambigu ⟹ AUCUNE trame sur les fils: prim={primary_frames:?} aux={aux_frames:?}"
        );
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 9 : resolve_aux_error_fallback_item — l'aux (mode
    /// `error-resolve` : completion normale, erreur sur les resolve) reçoit le
    /// resolve de SON item puis répond une erreur JSON-RPC ⟹ réponse client =
    /// item TEL QUEL (pas une erreur), session saine, réponse unique.
    #[tokio::test]
    async fn resolve_aux_error_fallback_item() {
        let (session, tmpdir) = make_merge_composite("", "error-resolve").await;
        let (client, mut rx) = session.subscribe();
        let merged = completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;
        assert_eq!(
            merged["result"],
            merged_completion_fixture(),
            "l'aux répond bien la completion en mode error-resolve (provenance écrite)"
        );

        let item = serde_json::json!({"label": "script", "kind": 3});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("fallback item après erreur de l'aux"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3));
        assert!(
            resp.get("error").is_none(),
            "l'erreur de l'aux ne devient JAMAIS une erreur globale: {resp}"
        );
        assert_eq!(
            resp["result"], item,
            "item renvoyé TEL QUEL (warn + no-op), sans enrichissement"
        );
        assert!(session.is_alive(), "la session survit à l'erreur de l'aux");

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 10 : resolve_aux_eof_fallback_item — l'aux (mode
    /// `hold-resolve-die`) retient le resolve de SON item puis MEURT sur
    /// `fake/kill` avec la requête en attente ⟹ purge EOF = fallback item tel
    /// quel, session saine, rien ne pend.
    #[tokio::test]
    async fn resolve_aux_eof_fallback_item() {
        let (session, tmpdir) = make_merge_composite("", "hold-resolve-die").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        let item = serde_json::json!({"label": "script", "kind": 3});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let is_resolve = is_method("completionItem/resolve");
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_resolve),
            Duration::from_secs(5),
        )
        .await;
        assert!(
            aux_frames.iter().any(is_resolve),
            "l'aux a reçu le resolve avant de mourir ; trames: {aux_frames:?}"
        );

        // L'aux en mode `hold-resolve-die` meurt sur `fake/kill` (fan-out de
        // la notification — invariant 2) AVOIR répondu.
        let kill = serde_json::json!({"jsonrpc": "2.0", "method": "fake/kill", "params": {}});
        session
            .send(client, kill.to_string().into_bytes())
            .await
            .expect("fake/kill ok");

        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("EOF aux avec resolve en attente ⟹ fallback, jamais un pend"),
        );
        assert_eq!(resp["id"].as_i64(), Some(3));
        assert!(
            resp.get("error").is_none(),
            "jamais une erreur globale: {resp}"
        );
        assert_eq!(resp["result"], item, "item TEL QUEL (fallback EOF)");
        assert!(session.is_alive(), "la session survit à l'EOF de l'aux");
        assert_eq!(session.pending_merge_count(), 0, "rien ne pend");

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");

        drop(tmpdir);
    }

    /// Tâche 08 test 11 : resolve_unsubscribe_mid_resolve — resolve (retenu
    /// par l'aux `hold-resolve`) puis `unsubscribe` ⟹ aucune réponse vers le
    /// client parti (même quand l'aux libère sa réponse retenue sur l'id
    /// interne purgé), pas de panique, AUCUN résidu : le resolve suivant d'un
    /// nouveau client sur un item PRIMAIRE aboutit normalement.
    #[tokio::test]
    async fn resolve_unsubscribe_mid_resolve() {
        let (session, tmpdir) = make_merge_composite("", "hold-resolve").await;
        let (client, mut rx) = session.subscribe();
        completion_setup(&session, client, &mut rx, "file:///w/App.vue").await;

        let item = serde_json::json!({"label": "script", "kind": 3});
        session
            .send(client, resolve_request(3, item.clone()))
            .await
            .expect("resolve ok");
        let is_resolve = is_method("completionItem/resolve");
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| frames.iter().any(is_resolve),
            Duration::from_secs(5),
        )
        .await;
        assert!(
            aux_frames.iter().any(is_resolve),
            "l'aux a reçu le resolve (il le retient) ; trames: {aux_frames:?}"
        );

        session.unsubscribe(client);

        // L'aux libère sa réponse retenue : id interne purgé par unsubscribe ⟹
        // avalée comme inconnue. Aucune réponse vers le client désabonné.
        let release = serde_json::json!({"jsonrpc": "2.0", "method": "x/release", "params": {}});
        session
            .send(client, release.to_string().into_bytes())
            .await
            .expect("x/release ok");
        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "aucune réponse vers un client désabonné: {leaked:?}"
        );

        // Aucun résidu observable : un nouveau client résout un item PRIMAIRE.
        let (client2, mut rx2) = session.subscribe();
        let tmpl = serde_json::json!({"label": "tmpl", "kind": 14});
        session
            .send(client2, resolve_request(9, tmpl.clone()))
            .await
            .expect("resolve 2 ok");
        let resp = client_msg(
            &recv_timeout(&mut rx2, Duration::from_secs(5))
                .await
                .expect("le resolve suivant (item primaire) doit aboutir normalement"),
        );
        assert_eq!(resp["id"].as_i64(), Some(9));
        assert_eq!(resp["result"], resolved(tmpl.clone(), "prim"));

        drop(tmpdir);
    }

    /// Tâche 08 test 12 : mono_completion_resolve_primary_unchanged —
    /// `aux: []` : completion ET resolve suivent le chemin historique strict
    /// (session ids 2 et 3 sur le fil — jamais les ids clients 42/43 — réponse
    /// verbatim/relayée, unique), et `completion_provenance` RESTE VIDE (la
    /// table n'est jamais consultée ni écrite en mono-process — invariant 8).
    #[tokio::test]
    async fn mono_completion_resolve_primary_unchanged() {
        // Mono-process construit à la main avec le fake primaire de fusion
        // (mode défaut) + son log recorder : le wire entrant est observable.
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join("merge_primary_mono.py");
        std::fs::write(&script_path, FAKE_LSP_MERGE_PRIMARY_PY).unwrap();
        let spec = LspToolchain {
            name: "mono".to_string(),
            bin: "python3".to_string(),
            args: vec![
                script_path.to_string_lossy().to_string(),
                primary_log_path(&tmpdir).to_string_lossy().to_string(),
            ],
            aux: vec![],
        };
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn mono-process ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(42, "textDocument/completion", "file:///w/main.rs", 0),
            )
            .await
            .expect("completion ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse completion du primaire"),
        );
        assert_eq!(resp["id"].as_i64(), Some(42));
        assert_eq!(
            resp["result"],
            prim_completion_list(),
            "completion VERBATIM du primaire (aucune fusion possible, table jamais consultée)"
        );

        let item = serde_json::json!({"label": "tmpl", "kind": 14});
        session
            .send(client, resolve_request(43, item.clone()))
            .await
            .expect("resolve ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("résultat du resolve par le primaire mono"),
        );
        assert_eq!(
            resp["id"].as_i64(),
            Some(43),
            "chemin historique : id restauré"
        );
        assert_eq!(
            resp["result"],
            resolved(item.clone(), "prim"),
            "le serveur mono a sa propre mémoire d'items — resolve primaire strict"
        );

        let is_completion = is_method("textDocument/completion");
        let is_resolve = is_method("completionItem/resolve");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_resolve),
            Duration::from_secs(5),
        )
        .await;
        let prim_completion = primary_frames
            .iter()
            .find(|f| is_completion(f))
            .expect("le primaire reçoit la completion");
        assert_eq!(
            prim_completion["id"].as_i64(),
            Some(2),
            "session id historique (initialize=1, completion=2), jamais l'id client 42"
        );
        let prim_resolve = primary_frames
            .iter()
            .find(|f| is_resolve(f))
            .expect("le primaire reçoit le resolve");
        assert_eq!(
            prim_resolve["id"].as_i64(),
            Some(3),
            "session id historique (…, resolve=3), jamais l'id client 43 ; trames: {primary_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponses uniques: {leaked:?}");
        assert_eq!(
            session.completion_provenance_uri_count(),
            0,
            "mono-process : provenance JAMAIS écrite"
        );
        assert_eq!(
            session.pending_merge_count(),
            0,
            "mono-process : aucune barrière"
        );

        drop(tmpdir);
    }

    /// Test 53: mono_process_wire_bytes_unchanged — `aux: []`, hover : le fil
    /// primaire porte la requête avec le SESSION id (réécriture historique
    /// intacte), réponse unique et id restauré ; AUCUN état de barrière créé
    /// avant/après (garde additionnelle de l'invariant 8 — les suites
    /// task-03/05/06 couvrent déjà le chemin mono de bout en bout).
    #[tokio::test]
    async fn mono_process_wire_bytes_unchanged() {
        // Mono-process construit à la main avec le recorder de la tâche 03 :
        // le wire entrant du primaire est observable, sa réponse est "ok".
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join("fake_lsp_mono.py");
        std::fs::write(&script_path, FAKE_LSP_RECORDER_PY).unwrap();
        let spec = LspToolchain {
            name: "mono".to_string(),
            bin: "python3".to_string(),
            args: vec![
                script_path.to_string_lossy().to_string(),
                primary_log_path(&tmpdir).to_string_lossy().to_string(),
            ],
            aux: vec![],
        };
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn mono-process ok");
        let (client, mut rx) = session.subscribe();
        assert_eq!(
            session.pending_merge_count(),
            0,
            "aucune barrière ne doit exister en mono-process"
        );

        // Premiers requêtes : initialize (session id 1) puis hover (session
        // id 2) avec un id client 42 qui ne doit JAMAIS passer sur le fil.
        composite_initialize(&session, client, &mut rx).await;
        session
            .send(
                client,
                text_request(42, "textDocument/hover", "file:///w/main.rs", 0),
            )
            .await
            .expect("hover ok");
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse unique du primaire"),
        );
        assert_eq!(
            resp["id"].as_i64(),
            Some(42),
            "id d'origine restauré (chemin historique)"
        );
        assert_eq!(resp["result"], serde_json::json!("ok"));

        let is_hover = is_method("textDocument/hover");
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| frames.iter().any(is_hover),
            Duration::from_secs(5),
        )
        .await;
        let prim = primary_frames
            .iter()
            .find(|f| is_hover(f))
            .expect("le primaire doit recevoir le hover");
        assert_eq!(
            prim["id"].as_i64(),
            Some(2),
            "fil primaire : session id historique (initialize=1, hover=2), jamais l'id client 42; trames: {primary_frames:?}"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(leaked.is_none(), "réponse unique: {leaked:?}");
        assert_eq!(
            session.pending_merge_count(),
            0,
            "le chemin mono ne crée jamais d'état de barrière"
        );

        drop(tmpdir);
    }

    /// Garde unitaire des fusionneurs (contrat tâche 07) : JAMAIS de panique
    /// sur JSON inattendu — la contribution d'une part malformée disparaît
    /// (vide + warn), et les cas explicites (`null` ⟹ `null` pour les
    /// locations/prepareRename, `null` ⟹ `[]` pour codeAction, tableau de
    /// marked strings à la sortie du repli de normalisation hover) sont
    /// exacts.
    #[test]
    fn merge_helpers_never_panic_on_unexpected_json() {
        let junk = vec![
            Some(serde_json::json!("pas un objet")),
            Some(serde_json::json!(42)),
            Some(serde_json::json!([1, 2])),
            Some(serde_json::json!({"contents": "kaboul"})),
            Some(serde_json::json!({"changes": "kaboul", "documentChanges": 7})),
            None,
            Some(Value::Null),
        ];
        for method in [
            "textDocument/hover",
            "textDocument/definition",
            "textDocument/typeDefinition",
            "textDocument/implementation",
            "textDocument/references",
            "textDocument/codeAction",
            "textDocument/prepareRename",
            "textDocument/rename",
            "textDocument/methode_inconnue",
        ] {
            let _merged = merge_result_for(method, &junk);
        }

        assert_eq!(
            merge_hover(&[None, Some(Value::Null)]),
            Value::Null,
            "hover : tous vides ⟹ null"
        );
        assert_eq!(
            merge_hover(&[Some(serde_json::json!({"contents": "x"})), None]),
            serde_json::json!({"contents": "x"}),
            "hover : une seule contribution ⟹ celle-là verbatim"
        );
        assert_eq!(
            merge_hover(&[
                Some(serde_json::json!({"contents": [{"language": "rust", "value": "code"}]})),
                Some(serde_json::json!({"contents": {"kind": "markdown", "value": "M"}})),
            ]),
            serde_json::json!({"contents": [
                {"language": "rust", "value": "code"},
                {"kind": "markdown", "value": "M"}
            ]}),
            "hover non-markup ×2 ⟹ tableau de marked strings primary-first"
        );
        assert_eq!(
            merge_locations(&[None, Some(Value::Null)]),
            Value::Null,
            "locations : toutes null ⟹ null (jamais [])"
        );
        assert_eq!(
            merge_locations(&[Some(Value::Null), Some(serde_json::json!([]))]),
            serde_json::json!([]),
            "locations : un tableau vu ⟹ tableau éventuellement vide"
        );
        assert_eq!(
            merge_code_actions(&[None, Some(Value::Null)]),
            serde_json::json!([]),
            "codeAction : null ⟹ []"
        );
        assert_eq!(
            merge_prepare_rename(&[Some(Value::Null), Some(serde_json::json!({"range": 1}))]),
            serde_json::json!({"range": 1}),
            "prepareRename : premier non-null gagne"
        );
        assert_eq!(
            merge_rename(&[None, Some(Value::Null)]),
            Value::Null,
            "rename : toutes null ⟹ null"
        );
        assert_eq!(
            merge_rename(&[
                Some(Value::Null),
                Some(serde_json::json!({"changes": {"u": []}}))
            ]),
            serde_json::json!({"changes": {"u": []}}),
            "rename : une seule contribution ⟹ celle-là"
        );
    }

    // ── Tâche 09 : semanticTokens — routage primaire + preuve bout-en-bout ──

    /// Script Python factice PRIMAIRE « vue-language-server tokens » (tâche 09) :
    /// journalise dans argv[1] ; sur `initialize` répond des capabilities
    /// standards (dont `semanticTokensProvider` — l'issue `initialize` est celle
    /// du primaire, tâche 03) ; mémorise l'URI du `didOpen` ; sur
    /// `textDocument/semanticTokens/full` émet D'ABORD
    /// `tsserver/request [[9, "_vue:encodedSemanticClassifications-full",
    /// {"file": uri, "start": 10}]]` (la plage `<script>` déléguée à tsserver en
    /// mode hybride Volar v3 — canal de forwarding de la tâche 05), attend la
    /// `tsserver/response` reçue sur son fil, puis répond UN flux de tokens
    /// UNIQUE : le token dérivé de la classification de l'aux (`body.spans[0]`
    /// mappé `[0, 0, longueur, classification]` — la preuve que le marqueur a
    /// traversé le canal) puis SON token template. Dégradation (body `null` ou
    /// timeout 2 s — le multiplexeur ne pend jamais le primaire, mais le faux
    /// ne doit jamais pendre non plus) : répond ses seuls tokens template, sans
    /// marqueur aux. Toute autre requête : réponse echo. Les lectures stdin
    /// sont crues (`os.read` + `select`) — sans quoi un buffer Python
    /// cacherait des octets au `select` et le timeout serait un mensonge.
    const FAKE_LSP_VUE_TOKENS_PY: &str = r#"
import sys, os, json, time, select

LOG = sys.argv[1] if len(sys.argv) > 1 else ""

# token template émis par le primaire lui-même (legend factice : tokenType 1)
TEMPLATE_TOKEN = [50, 0, 4, 1]

doc_uri = ""
# None, ou {"req_id": …, "deadline": …} en attente de la tsserver/response
awaiting = None

def read_frame(deadline=None):
    # -> (data, timed_out) ; data None = EOF ; timed_out seulement si deadline
    # posée (select sur le fd 0, lecture non bufferisée).
    header = b""
    while True:
        if deadline is not None and not select.select([0], [], [], max(0.0, deadline - time.monotonic()))[0]:
            return None, True
        ch = os.read(0, 1)
        if not ch:
            return None, False
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b"", False
    data = b""
    while len(data) < length:
        if deadline is not None and not select.select([0], [], [], max(0.0, deadline - time.monotonic()))[0]:
            return None, True
        chunk = os.read(0, length - len(data))
        if not chunk:
            break
        data += chunk
    return data, False

def write_frame(obj):
    out = json.dumps(obj).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(out)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(out)
    sys.stdout.buffer.flush()

def answer_tokens(req_id, aux_body):
    data = []
    try:
        spans = aux_body["spans"]
        if spans:
            data += [0, 0, spans[0][1], spans[0][2]]
    except Exception:
        pass
    data += TEMPLATE_TOKEN
    write_frame({"jsonrpc": "2.0", "id": req_id, "result": {"data": data}})

while True:
    deadline = awaiting["deadline"] if awaiting else None
    raw, timed_out = read_frame(deadline)
    if timed_out:
        # aucune réponse dans les 2 s (jamais en pratique : le multiplexeur
        # répond `body null` à toute tsserver/request — contrat tâche 05)
        req_id = awaiting["req_id"]
        awaiting = None
        answer_tokens(req_id, None)
        continue
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
    if method == "tsserver/response":
        if awaiting is not None:
            try:
                body = msg["params"][0][1]
            except Exception:
                body = None
            req_id = awaiting["req_id"]
            awaiting = None
            answer_tokens(req_id, body)
        continue
    if method == "textDocument/didOpen":
        doc_uri = msg.get("params", {}).get("textDocument", {}).get("uri", "")
        continue
    if method == "textDocument/semanticTokens/full" and "id" in msg:
        write_frame({"jsonrpc": "2.0", "method": "tsserver/request",
                     "params": [[9, "_vue:encodedSemanticClassifications-full",
                                 {"file": doc_uri, "start": 10}]]})
        awaiting = {"req_id": msg["id"], "deadline": time.monotonic() + 2.0}
        continue
    if "id" in msg:
        if method == "initialize":
            write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
                "semanticTokensProvider": {
                    "legend": {"tokenTypes": ["template", "aux"], "tokenModifiers": []},
                    "full": True}}}})
        else:
            write_frame({"jsonrpc": "2.0", "id": msg["id"], "result": {"echo": method}})
"#;

    /// Script Python factice aux « typescript-language-server classifier »
    /// (tâche 09) : journalise dans argv[1] ; sur `initialize` répond
    /// `{"capabilities":{}}` ; sur `workspace/executeCommand` avec
    /// `arguments[0] == "_vue:encodedSemanticClassifications-full"`, répond
    /// l'objet tsserver complet `{"type":"response","success":true,
    /// "body":{"spans":[[10,3,7]]}}` (forme encoded classification tsserver :
    /// tableau plat de spans `[start, longueur, classification]`) ; toute autre
    /// commande forwardée : `body: null`. Modes via `fake_mode` : `"error"`
    /// (JSON-RPC erreur — l'aux en échec de la délégation).
    const FAKE_LSP_TLS_CLASSIFY_PY: &str = r#"
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
        continue
    args = msg.get("params", {}).get("arguments", [])
    cmd = args[0] if len(args) > 0 else None
    if cmd == "_vue:encodedSemanticClassifications-full":
        write_frame({"jsonrpc": "2.0", "id": msg["id"],
                     "result": {"type": "response", "success": True,
                                "body": {"spans": [[10, 3, 7]]}}})
    else:
        write_frame({"jsonrpc": "2.0", "id": msg["id"],
                     "result": {"type": "response", "success": True, "body": None}})
"#;

    /// Trames d'un log dont la méthode mentionne `semanticTokens` (garde de
    /// silence du fil aux : les méthodes semanticTokens ne sortent jamais du
    /// primaire).
    fn semtok_frames(frames: &[Value]) -> Vec<&Value> {
        frames
            .iter()
            .filter(|f| {
                f.get("method")
                    .and_then(|m| m.as_str())
                    .is_some_and(|m| m.contains("semanticTokens"))
            })
            .collect()
    }

    /// Tâche 09 test 1 : `semtok_routes_primary_only_end_to_end` — le client
    /// envoie `semanticTokens/full` (id 4) : (a) le fil client reçoit UNE seule
    /// réponse id 4 dont `data` porte ET le token template du primaire ET le
    /// marqueur dérivé de la classification de l'aux (preuve bout-en-bout de la
    /// délégation par plage) ; (b) le log aux contient l'`executeCommand`
    /// `_vue:encodedSemanticClassifications-full` et AUCUNE trame
    /// `semanticTokens/*` ; (c) le primaire a reçu la `tsserver/response` avec
    /// le body dépilé.
    #[tokio::test]
    async fn semtok_routes_primary_only_end_to_end() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUE_TOKENS_PY,
            &[("tsserver-forward", FAKE_LSP_TLS_CLASSIFY_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;
        composite_open(&session, client, "file:///w/App.vue").await;
        session
            .send(
                client,
                text_request(
                    4,
                    "textDocument/semanticTokens/full",
                    "file:///w/App.vue",
                    0,
                ),
            )
            .await
            .expect("semanticTokens/full ok");

        // (b) l'aux reçoit l'executeCommand de classification — et côté fil aux
        // ne JAMAIS circuler de semanticTokens/* (le flux de tokens ne s'est
        // jamais scindé).
        let aux_frames = poll_recorder(
            &aux_log_path(&tmpdir, 0),
            |frames| forwarded_execute_command(frames).is_some(),
            Duration::from_secs(5),
        )
        .await;
        let exec = forwarded_execute_command(&aux_frames)
            .expect("l'aux doit recevoir l'executeCommand de classification");
        assert_eq!(
            exec["params"]["arguments"],
            serde_json::json!([
                "_vue:encodedSemanticClassifications-full",
                {"file": "file:///w/App.vue", "start": 10}
            ]),
            "arguments = [command, payload {{file, start}}] exactement ; trames: {aux_frames:?}"
        );
        assert!(
            semtok_frames(&aux_frames).is_empty(),
            "aucune trame semanticTokens/* ne doit atteindre l'aux ; trames: {aux_frames:?}"
        );

        // (c) le primaire reçoit la tsserver/response [[9, body DÉPILÉ]], le
        // vue-id restitué tel quel.
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[9, {"spans": [[10, 3, 7]]}]])],
            "params = [[vue_id 9, body dépilé de l'objet tsserver complet]] ; trames: {primary_frames:?}"
        );

        // (a) le client reçoit UNE réponse id 4 au flux unique : token dérivé
        // des classifications aux (span [10,3,7] ⟹ token [0,0,3,7]) + token
        // template du primaire.
        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse unique semanticTokens/full"),
        );
        assert_eq!(resp["id"].as_i64(), Some(4));
        assert_eq!(
            resp["result"],
            serde_json::json!({"data": [0, 0, 3, 7, 50, 0, 4, 1]}),
            "marqueur issu des classifications aux + token template, dans UN flux unique"
        );

        let leaked = recv_timeout(&mut rx, Duration::from_millis(500)).await;
        assert!(
            leaked.is_none(),
            "réponse unique du primaire, aucune trame tsserver/* ni part aux sur le fil client: {leaked:?}"
        );

        drop(tmpdir);
    }

    /// Tâche 09 test 2 : `semtok_forward_failure_degrades_to_template_tokens` —
    /// aux en mode erreur : le forwarding répond `body null` au primaire
    /// (contrat tâche 05) et le faux primaire répond ses seuls tokens template
    /// ⟹ le client reçoit une réponse de tokens VALIDE, sans marqueur aux,
    /// sans aucune erreur JSON-RPC.
    #[tokio::test]
    async fn semtok_forward_failure_degrades_to_template_tokens() {
        let aux_script = fake_mode(FAKE_LSP_TLS_CLASSIFY_PY, "error");
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUE_TOKENS_PY,
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
        session
            .send(
                client,
                text_request(
                    4,
                    "textDocument/semanticTokens/full",
                    "file:///w/App.vue",
                    0,
                ),
            )
            .await
            .expect("semanticTokens/full ok");

        // le primaire a bien reçu le body null de la dégradation (canal tâche 05)
        let primary_frames = poll_recorder(
            &primary_log_path(&tmpdir),
            |frames| !tsserver_response_params(frames).is_empty(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            tsserver_response_params(&primary_frames),
            vec![serde_json::json!([[9, null]])],
            "erreur de l'aux ⟹ body null au primaire ; trames: {primary_frames:?}"
        );

        let resp = client_msg(
            &recv_timeout(&mut rx, Duration::from_secs(5))
                .await
                .expect("réponse de tokens valide même aux en échec"),
        );
        assert_eq!(resp["id"].as_i64(), Some(4));
        assert!(
            resp.get("error").is_none(),
            "aucune erreur JSON-RPC ne doit atteindre le client: {resp}"
        );
        assert_eq!(
            resp["result"],
            serde_json::json!({"data": [50, 0, 4, 1]}),
            "seuls les tokens template du primaire, sans marqueur aux"
        );
        assert!(
            session.is_alive(),
            "l'échec de la délégation ne tue pas la session"
        );

        drop(tmpdir);
    }

    /// Tâche 09 test 3 : `semtok_table_explicit_primary` — assert unitaire
    /// direct de la table sur les 4 méthodes semanticTokens :
    /// `MergeRoute::Primary` est une décision documentée (cas explicites du
    /// `match`), pas un accident du défaut ; `hover` reste `All` (garde de
    /// non-régression de la table).
    #[test]
    fn semtok_table_explicit_primary() {
        for method in [
            "textDocument/semanticTokens/full",
            "textDocument/semanticTokens/range",
            "textDocument/semanticTokens/delta",
            "workspace/semanticTokens/refresh",
        ] {
            assert_eq!(
                merge_route_for_method(method),
                MergeRoute::Primary,
                "{method} : routage PRIMAIRE explicite — jamais de fusion de flux de tokens"
            );
        }
        assert_eq!(
            merge_route_for_method("textDocument/hover"),
            MergeRoute::All,
            "garde : les cas All de la table sont intacts"
        );
    }

    /// Tâche 09 test 4 : `semtok_range_and_delta_wire_quiet` —
    /// `semanticTokens/range` et `semanticTokens/delta` émis par le client : la
    /// réponse primaire est servie verbatim (id restauré, aucune barrière), et
    /// le log aux ne porte AUCUNE trame `semanticTokens` — jamais vus par lui.
    #[tokio::test]
    async fn semtok_range_and_delta_wire_quiet() {
        let (spec, tmpdir) = make_fake_composite(
            FAKE_LSP_VUE_TOKENS_PY,
            &[("tsserver-forward", FAKE_LSP_TLS_CLASSIFY_PY, vec![])],
        )
        .await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root)
            .await
            .expect("spawn composite ok");
        let (client, mut rx) = session.subscribe();

        composite_initialize(&session, client, &mut rx).await;

        for (id, method) in [
            (5, "textDocument/semanticTokens/range"),
            (6, "textDocument/semanticTokens/delta"),
        ] {
            session
                .send(client, text_request(id, method, "file:///w/App.vue", 0))
                .await
                .expect("requête semanticTokens ok");
            let resp = client_msg(
                &recv_timeout(&mut rx, Duration::from_secs(5))
                    .await
                    .unwrap_or_else(|| panic!("la réponse {method} du primaire doit être servie")),
            );
            assert_eq!(resp["id"].as_i64(), Some(id), "id restauré pour {method}");
            assert_eq!(
                resp["result"],
                serde_json::json!({"echo": method}),
                "réponse primaire servie verbatim (pas de barrière ni de fusion pour {method})"
            );
        }

        let aux_frames = read_recorder_frames(&aux_log_path(&tmpdir, 0));
        assert!(
            semtok_frames(&aux_frames).is_empty(),
            "l'aux ne voit jamais range/delta ; trames: {aux_frames:?}"
        );
        assert_eq!(
            session.pending_merge_count(),
            0,
            "aucune barrière de fusion créée par les méthodes semanticTokens"
        );

        drop(tmpdir);
    }
}
