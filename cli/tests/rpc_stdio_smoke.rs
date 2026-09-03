use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::json;
use tempfile::tempdir;

#[test]
fn initialize_then_shutdown_end_to_end() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn vanyline serve --stdio");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1}}}}"#
    )
    .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["result"]["protocolVersion"], 1);

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"shutdown"}}"#).unwrap();
    let mut line2 = String::new();
    reader.read_line(&mut line2).unwrap();
    let resp2: serde_json::Value = serde_json::from_str(&line2).unwrap();
    assert_eq!(resp2["result"], serde_json::Value::Null);

    let status = child.wait().expect("process did not exit");
    assert!(status.success());
}

// ---------------------------------------------------------------------------
// Helpers — client ndjson minimal pour tests process (tâche 4a)
// ---------------------------------------------------------------------------

/// Client ndjson minimal pour un test : une requête en attente à la fois
/// (jamais de pipelining — pas de risque de saturation du pipe stdout).
struct Client {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Client {
    /// Spawn `vanyline serve --stdio` avec `XDG_CONFIG_HOME = XDG_DATA_HOME =
    /// home` (tmpdir dédié au test — la config réelle de la machine n'est
    /// JAMAIS touchée ni lue ; l'env du process de test n'est pas modifié non
    /// plus, l'injection est faite sur le child uniquement via `Command::env`).
    /// Puis `initialize` (id 0) avec `workspace` seulement si fourni, et
    /// assertion d'absence d'`error` dans la réponse.
    fn spawn(home: &Path, workspace: Option<&Path>) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vanyline"))
            .args(["serve", "--stdio"])
            .env("XDG_CONFIG_HOME", home)
            .env("XDG_DATA_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn vanyline serve --stdio");

        let mut params = json!({ "protocolVersion": 1 });
        if let Some(ws) = workspace {
            params["workspace"] = json!(ws.display().to_string());
        }

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);
        let mut client = Client {
            child,
            stdin,
            reader,
            next_id: 0,
        };
        let resp = client.call("initialize", params);
        assert_eq!(
            Client::vnl_code(&resp),
            None,
            "initialize should succeed, got: {resp}"
        );
        client
    }

    /// Une requête -> une ligne de réponse (lecture synchrone bloquante,
    /// pattern `read_line` du test existant). Retourne la réponse complète
    /// parseée (Value avec `result` et/ou `error`). id auto-incrémenté.
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").unwrap();
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .unwrap_or_else(|e| panic!("failed to read response to {method} (id {id}): {e}"));
        assert!(
            read > 0,
            "server closed stdout before answering {method} (id {id})"
        );
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("invalid JSON response to {method} (id {id}): {e} — {line}"))
    }

    /// Enveloppe `error.data.code` de la réponse, ou `None` si succès.
    fn vnl_code(response: &serde_json::Value) -> Option<String> {
        response["error"]["data"]["code"].as_str().map(String::from)
    }

    /// `shutdown` + attente exit status success. À appeler à la fin de
    /// chaque test (sinon le child reste vivant jusqu'au drop du test).
    fn shutdown(mut self) {
        let resp = self.call("shutdown", serde_json::Value::Null);
        assert_eq!(
            resp["result"],
            serde_json::Value::Null,
            "shutdown should answer result:null, got: {resp}"
        );
        let status = self.child.wait().expect("process did not exit");
        assert!(
            status.success(),
            "vanyline serve --stdio should exit successfully after shutdown"
        );
    }
}

/// Une lecture `<list_method>` : succès asserté, retourne le tableau des
/// entrées telles que sérialisées par le binaire réel.
fn list_entries(c: &mut Client, list_method: &str) -> Vec<serde_json::Value> {
    let resp = c.call(list_method, serde_json::Value::Null);
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "{list_method} should succeed, got: {resp}"
    );
    resp["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{list_method} result should be an array, got: {resp}"))
        .clone()
}

/// Liste puis retourne l'entrée `name == name` — panic explicite si absente.
fn find_entry(c: &mut Client, list_method: &str, name: &str) -> serde_json::Value {
    list_entries(c, list_method)
        .into_iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("{name} should be listed by {list_method}"))
}

/// Round-trip complet d'un domaine au-dessus de `call` :
/// 1. `<domain>/create` (params = enveloppe complète `{item, [body], [layer]}`)
///    -> succès (`result: null`).
/// 2. `<list_method>` -> tableau contenant un objet avec `name == name`.
/// 3. `<domain>/update` `{name, patch}` -> succès.
/// 4. `<list_method>` -> entrée contient `name` et le champ `changed.0 ==
///    changed.1` (le patch est visible, sérialisé par le binaire réel).
/// 5. `<domain>/delete` `{name}` -> succès.
/// 6. `<list_method>` -> plus aucune entrée avec `name == name`.
fn roundtrip(
    c: &mut Client,
    domain: &str,      // ex. "config/providers"
    list_method: &str, // ex. "config/providers"
    name: &str,
    create_params: serde_json::Value,
    update_patch: serde_json::Value,
    changed: (&str, serde_json::Value),
) {
    // 1. create -> succès `result: null`
    let resp = c.call(&format!("{domain}/create"), create_params);
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "{domain}/create should succeed, got: {resp}"
    );
    assert_eq!(
        resp["result"],
        serde_json::Value::Null,
        "{domain}/create should answer result:null, got: {resp}"
    );

    // 2. l'entrée est listée après le create
    find_entry(c, list_method, name);

    // 3. update (patch partiel) -> succès `result: null`
    let resp = c.call(
        &format!("{domain}/update"),
        json!({ "name": name, "patch": update_patch }),
    );
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "{domain}/update should succeed, got: {resp}"
    );
    assert_eq!(
        resp["result"],
        serde_json::Value::Null,
        "{domain}/update should answer result:null, got: {resp}"
    );

    // 4. le patch est visible dans la liste relue
    let entry = find_entry(c, list_method, name);
    assert_eq!(
        entry[changed.0], changed.1,
        "{domain}/update patch should be visible on {name}, entry: {entry}"
    );

    // 5. delete -> succès `result: null`
    let resp = c.call(&format!("{domain}/delete"), json!({ "name": name }));
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "{domain}/delete should succeed, got: {resp}"
    );
    assert_eq!(
        resp["result"],
        serde_json::Value::Null,
        "{domain}/delete should answer result:null, got: {resp}"
    );

    // 6. plus aucune entrée avec ce nom
    let entries = list_entries(c, list_method);
    assert!(
        entries.iter().all(|entry| entry["name"] != name),
        "{name} should be gone from {list_method} after delete, got: {entries:?}"
    );
}

// ---------------------------------------------------------------------------
// Round-trips 6 domaines (create -> list -> update -> list -> delete -> list)
// ---------------------------------------------------------------------------

#[test]
fn smoke_roundtrip_providers() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let create_params = json!({
        "item": { "name": "prov-s", "type": "ollama", "endpoint": "http://localhost:11434" }
    });
    let patch = json!({ "endpoint": "http://smoke:1" });
    roundtrip(
        &mut c,
        "config/providers",
        "config/providers",
        "prov-s",
        create_params.clone(),
        patch.clone(),
        ("endpoint", json!("http://smoke:1")),
    );

    // Le round-trip complet a supprimé l'entrée à l'étape 6 : on rejoue
    // create + update pour vérifier qu'un champ non patché survit au patch
    // partiel (sérialisé par le binaire réel).
    c.call("config/providers/create", create_params);
    c.call(
        "config/providers/update",
        json!({ "name": "prov-s", "patch": patch }),
    );
    let entry = find_entry(&mut c, "config/providers", "prov-s");
    assert_eq!(
        entry["type"], "ollama",
        "unpatched type should survive the partial patch, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_roundtrip_models() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let create_params = json!({
        "item": { "name": "m-s", "provider": "prov-s", "model": "llama3" }
    });
    let patch = json!({ "model": "smoke-model" });
    roundtrip(
        &mut c,
        "config/models",
        "config/models",
        "m-s",
        create_params.clone(),
        patch.clone(),
        ("model", json!("smoke-model")),
    );

    c.call("config/models/create", create_params);
    c.call(
        "config/models/update",
        json!({ "name": "m-s", "patch": patch }),
    );
    let entry = find_entry(&mut c, "config/models", "m-s");
    assert_eq!(
        entry["provider"], "prov-s",
        "unpatched provider should survive the partial patch, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_roundtrip_mcp_servers() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let create_params = json!({
        "item": { "name": "srv-s", "type": "http-streamable", "url": "http://x/mcp" }
    });
    let patch = json!({ "url": "http://y/mcp" });
    roundtrip(
        &mut c,
        "config/mcpServers",
        "config/mcpServers",
        "srv-s",
        create_params.clone(),
        patch.clone(),
        ("url", json!("http://y/mcp")),
    );

    c.call("config/mcpServers/create", create_params);
    c.call(
        "config/mcpServers/update",
        json!({ "name": "srv-s", "patch": patch }),
    );
    let entry = find_entry(&mut c, "config/mcpServers", "srv-s");
    assert_eq!(
        entry["type"], "http-streamable",
        "unpatched type should survive the partial patch, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_roundtrip_toolsets() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let create_params = json!({
        "item": { "name": "ts-s", "local_tools": ["read_file"] }
    });
    let patch = json!({ "description": "patchée" });
    roundtrip(
        &mut c,
        "config/toolsets",
        "config/toolsets",
        "ts-s",
        create_params.clone(),
        patch.clone(),
        ("description", json!("patchée")),
    );

    c.call("config/toolsets/create", create_params);
    c.call(
        "config/toolsets/update",
        json!({ "name": "ts-s", "patch": patch }),
    );
    let entry = find_entry(&mut c, "config/toolsets", "ts-s");
    assert_eq!(
        entry["local_tools"],
        json!(["read_file"]),
        "unpatched local_tools should survive the partial patch, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_roundtrip_agents() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let create_params = json!({
        "item": { "name": "ag-s", "model": "m", "system_prompt": "p" }
    });
    let patch = json!({ "model": "m2" });
    roundtrip(
        &mut c,
        "config/agents",
        "config/agents",
        "ag-s",
        create_params.clone(),
        patch.clone(),
        ("model", json!("m2")),
    );

    c.call("config/agents/create", create_params);
    c.call(
        "config/agents/update",
        json!({ "name": "ag-s", "patch": patch }),
    );
    let entry = find_entry(&mut c, "config/agents", "ag-s");
    assert_eq!(
        entry["system_prompt"], "p",
        "unpatched system_prompt should survive the partial patch, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_roundtrip_skills() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // create porte `item` (SkillMeta) + `body` séparé ; la lecture
    // `config/skills` n'expose que name/description — le body n'est JAMAIS
    // asserté en lecture (pas de méthode de load côté RPC, cf. design).
    roundtrip(
        &mut c,
        "config/skills",
        "config/skills",
        "sk-s",
        json!({
            "item": { "name": "sk-s", "description": "skill smoke" },
            "body": "corps initial"
        }),
        json!({ "description": "skill patché" }),
        ("description", json!("skill patché")),
    );

    c.shutdown();
}

// ---------------------------------------------------------------------------
// Lectures seedées depuis un config.yaml global écrit à la main
// ---------------------------------------------------------------------------

#[test]
fn smoke_reads_seeded_global_config() {
    let home = tempdir().unwrap();
    // Workspace SANS marqueur `.vanyline`/`.git` -> workspace non résolu
    // (pattern du test unitaire `initialize_no_workspace_marker_yields_none`) :
    // la lecture vient du seul couche globale seedée ci-dessous.
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("vanyline")).unwrap();
    std::fs::write(
        home.path().join("vanyline").join("config.yaml"),
        "\
providers:
  prov-seed:
    type: ollama
    endpoint: http://seed:11434
mcp:
  srv-seed:
    type: http-streamable
    url: http://seed:3000
",
    )
    .unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let provider = find_entry(&mut c, "config/providers", "prov-seed");
    assert_eq!(
        provider["type"], "ollama",
        "seeded provider type, entry: {provider}"
    );
    assert_eq!(
        provider["endpoint"], "http://seed:11434",
        "seeded provider endpoint, entry: {provider}"
    );

    let server = find_entry(&mut c, "config/mcpServers", "srv-seed");
    assert_eq!(
        server["type"], "http-streamable",
        "seeded mcp server type, entry: {server}"
    );
    assert_eq!(
        server["url"], "http://seed:3000",
        "seeded mcp server url, entry: {server}"
    );

    c.shutdown();
}

// ---------------------------------------------------------------------------
// Codes d'erreur config (VNL-RPC-012..015) + cible de couche (tâche 4b)
// ---------------------------------------------------------------------------

#[test]
fn smoke_conflict_returns_013() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // providers (couche workspace par défaut) : create deux fois le même
    // name dans la même couche -> NameConflict côté store -> 013.
    let item = json!({ "name": "prov-c", "type": "ollama", "endpoint": "http://x" });
    let resp = c.call("config/providers/create", json!({ "item": item.clone() }));
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "first providers/create should succeed, got: {resp}"
    );
    let resp = c.call("config/providers/create", json!({ "item": item }));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-013"),
        "duplicate providers/create should be VNL-RPC-013, got: {resp}"
    );

    // agents (domaine fichier, représentatif des 3 domaines fichiers)
    let item = json!({ "name": "ag-c", "model": "m", "system_prompt": "p" });
    let resp = c.call("config/agents/create", json!({ "item": item.clone() }));
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "first agents/create should succeed, got: {resp}"
    );
    let resp = c.call("config/agents/create", json!({ "item": item }));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-013"),
        "duplicate agents/create should be VNL-RPC-013, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_not_found_returns_012() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // update/delete sur un name absent de la couche ciblée -> 012 (la
    // suppression de config n'est PAS idempotente — design).
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "config/providers/update",
            json!({ "name": "ghost", "patch": { "endpoint": "http://y" } }),
        ),
        ("config/providers/delete", json!({ "name": "ghost" })),
        (
            "config/agents/update",
            json!({ "name": "ghost", "patch": { "model": "m2" } }),
        ),
        ("config/agents/delete", json!({ "name": "ghost" })),
    ];
    for (method, params) in cases {
        let resp = c.call(method, params);
        assert_eq!(
            Client::vnl_code(&resp).as_deref(),
            Some("VNL-RPC-012"),
            "{method} on an absent name should be VNL-RPC-012, got: {resp}"
        );
    }

    c.shutdown();
}

#[test]
fn smoke_invalid_names_return_014() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // Items agents VALIDES (model + system_prompt présents) — sinon la
    // désérialisation `item` échouerait et répondrait 015 AVANT la validation
    // du nom côté store. Les 4 noms anti-traversal du design, `/abs` reste
    // une chaîne littérale dans le JSON.
    for name in ["../evil", "a/b", "..", "/abs"] {
        let resp = c.call(
            "config/agents/create",
            json!({ "item": { "name": name, "model": "m", "system_prompt": "p" } }),
        );
        assert_eq!(
            Client::vnl_code(&resp).as_deref(),
            Some("VNL-RPC-014"),
            "agents/create with name {name:?} should be VNL-RPC-014, got: {resp}"
        );
    }

    // Assert FS (pattern du test unitaire 3b) : aucune écriture disque — le
    // workspace ne contient que le marqueur `.vanyline`, lui-même vide (pas
    // de sous-répertoire `agents/` créé par les tentatives rejetées).
    let ws_entries: Vec<std::ffi::OsString> = std::fs::read_dir(ws.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        ws_entries,
        vec![std::ffi::OsString::from(".vanyline")],
        "invalid names must not create anything in the workspace"
    );
    let inner_entries: Vec<std::ffi::OsString> = std::fs::read_dir(ws.path().join(".vanyline"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        inner_entries.is_empty(),
        "no agents/ dir or file should exist under .vanyline, found: {inner_entries:?}"
    );

    c.shutdown();
}

#[test]
fn smoke_bad_provider_type_returns_015() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // type hors enum ProviderType -> échec de désérialisation `item` côté
    // handler -> VNL-RPC-015 (avant même d'atteindre le store).
    let resp = c.call(
        "config/providers/create",
        json!({ "item": { "name": "prov-b", "type": "bogus", "endpoint": "http://x" } }),
    );
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-015"),
        "create with an unknown provider type should be VNL-RPC-015, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_layer_targets_isolated() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    // `<home>/vanyline/` n'est PAS pré-créé : le store doit le créer
    // lui-même sur l'écriture `layer:"global"` (comportement
    // `set_default_agent_creates_missing_global_dir`).
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let global_file = home.path().join("vanyline").join("config.yaml");

    // create global -> le store crée <home>/vanyline/config.yaml avec prov-g
    let resp = c.call(
        "config/providers/create",
        json!({
            "layer": "global",
            "item": { "name": "prov-g", "type": "ollama", "endpoint": "http://g" }
        }),
    );
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "global providers/create should succeed, got: {resp}"
    );
    assert!(
        global_file.is_file(),
        "layer:global create should create {} itself",
        global_file.display()
    );
    let before = std::fs::read(&global_file).unwrap();
    assert!(
        String::from_utf8_lossy(&before).contains("prov-g"),
        "global config.yaml should contain prov-g, content: {}",
        String::from_utf8_lossy(&before)
    );

    // create workspace
    let resp = c.call(
        "config/providers/create",
        json!({
            "layer": "workspace",
            "item": { "name": "prov-w", "type": "ollama", "endpoint": "http://w" }
        }),
    );
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "workspace providers/create should succeed, got: {resp}"
    );

    // Le fichier global est identique octet pour octet : l'écriture
    // workspace n'a pas re-sérialisé/touché le global.
    let after = std::fs::read(&global_file).unwrap();
    assert_eq!(
        before, after,
        "workspace write must not touch the global config.yaml"
    );

    let ws_file = ws.path().join(".vanyline").join("config.yaml");
    let ws_before = std::fs::read(&ws_file).unwrap();
    let ws_content = String::from_utf8_lossy(&ws_before).into_owned();
    assert!(
        ws_content.contains("prov-w"),
        "workspace config.yaml should contain prov-w, content: {ws_content}"
    );
    assert!(
        !ws_content.contains("prov-g"),
        "workspace config.yaml must not contain prov-g, content: {ws_content}"
    );

    // Lecture fusionnée : les deux couches visibles
    find_entry(&mut c, "config/providers", "prov-g");
    find_entry(&mut c, "config/providers", "prov-w");

    // delete ciblé global : prov-g disparaît du global, prov-w survit
    let resp = c.call(
        "config/providers/delete",
        json!({ "layer": "global", "name": "prov-g" }),
    );
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "global providers/delete should succeed, got: {resp}"
    );
    let global_after_delete = std::fs::read_to_string(&global_file).unwrap();
    assert!(
        !global_after_delete.contains("prov-g"),
        "global config.yaml should no longer contain prov-g, content: {global_after_delete}"
    );
    find_entry(&mut c, "config/providers", "prov-w");
    assert_eq!(
        std::fs::read(&ws_file).unwrap(),
        ws_before,
        "global delete must not touch the workspace config.yaml"
    );

    c.shutdown();
}

// ---------------------------------------------------------------------------
// Registre statique des tools intégrés (tâche 5a)
// ---------------------------------------------------------------------------

#[test]
fn smoke_local_tools_returns_eight() {
    // Registre statique : le test ne dépend pas de la config — le préambule
    // standard (home/ws tmpdirs, marqueur `.vanyline/`) est conservé pour
    // l'isolation (pattern des tâches 4a/4b).
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let resp = c.call("config/localTools", serde_json::Value::Null);
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "config/localTools should succeed, got: {resp}"
    );
    let tools = resp["result"]
        .as_array()
        .unwrap_or_else(|| panic!("config/localTools result should be an array, got: {resp}"));
    assert_eq!(
        tools.len(),
        8,
        "config/localTools should list the 8 builtin tools, got: {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "execute_command"),
        "localTools should contain execute_command, got: {tools:?}"
    );

    c.shutdown();
}

// ---------------------------------------------------------------------------
// Actions réseau — config/providers/test & config/mcpServers/test (tâche 5b)
// Chemins d'erreur uniquement (pas de serveur LLM/MCP réel) : nom inconnu,
// endpoint injoignable, et — pour MCP — cible qui accepte la connexion sans
// jamais répondre (valide le timeout : le dispatch RPC est série, un hang
// gèlerait tout le serveur).
// ---------------------------------------------------------------------------

#[test]
fn smoke_provider_test_unknown_name_returns_006() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let resp = c.call("config/providers/test", json!({ "name": "does-not-exist" }));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-006"),
        "provider/test on unknown name should be VNL-RPC-006, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_provider_test_unreachable_endpoint_returns_006() {
    // Port fermé garanti : on bind puis on relâche immédiatement.
    let closed_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    c.call(
        "config/providers/create",
        json!({ "item": {
            "name": "prov-dead",
            "type": "ollama",
            "endpoint": format!("http://127.0.0.1:{closed_port}"),
        }}),
    );

    let resp = c.call("config/providers/test", json!({ "name": "prov-dead" }));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-006"),
        "provider/test on an unreachable endpoint should be VNL-RPC-006, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_mcp_test_unknown_name_returns_006() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    let resp = c.call(
        "config/mcpServers/test",
        json!({ "name": "does-not-exist" }),
    );
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-006"),
        "mcpServers/test on unknown name should be VNL-RPC-006, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_mcp_test_black_hole_times_out_without_hanging() {
    // Listener qui accepte la connexion TCP mais ne renvoie JAMAIS de réponse
    // HTTP — le client MCP streamable-http n'a aucun timeout propre. Sans le
    // timeout côté handler, ce test bloquerait indéfiniment (et gèlerait le
    // serveur RPC en prod). Avec, la réponse arrive à ~10 s.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        // Garde les connexions ouvertes sans répondre, le temps du test.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        listener
            .set_nonblocking(true)
            .expect("set_nonblocking on black-hole listener");
        let mut held = Vec::new();
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => held.push(stream), // jamais lu/écrit
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    });

    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    c.call(
        "config/mcpServers/create",
        json!({ "item": {
            "name": "srv-blackhole",
            "type": "http-streamable",
            "url": format!("http://127.0.0.1:{port}/mcp"),
        }}),
    );

    let started = std::time::Instant::now();
    let resp = c.call("config/mcpServers/test", json!({ "name": "srv-blackhole" }));
    let elapsed = started.elapsed();

    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-006"),
        "mcpServers/test on a black hole should error VNL-RPC-006, got: {resp}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(25),
        "mcpServers/test must not hang — returned in {elapsed:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(8),
        "the ~10s handler timeout should be what ends this call, not a fast \
         failure — returned in {elapsed:?}"
    );
    // Le serveur reste vivant et répond à la requête suivante.
    let follow = c.call("config/providers", serde_json::Value::Null);
    assert_eq!(
        Client::vnl_code(&follow),
        None,
        "server should still answer after a timed-out mcp test, got: {follow}"
    );

    c.shutdown();
    drop(handle); // le thread se termine seul via sa deadline
}

// ---------------------------------------------------------------------------
// config/skills/get — lecture du body d'un skill (F4 tâche 01)
// ---------------------------------------------------------------------------

#[test]
fn smoke_skills_get_returns_body_and_source() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // create sans `layer` -> couche workspace (pattern de smoke_roundtrip_skills)
    let resp = c.call(
        "config/skills/create",
        json!({
            "item": { "name": "sk-get", "description": "d" },
            "body": "ligne 1\nligne 2"
        }),
    );
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "skills/create should succeed, got: {resp}"
    );

    // get -> {name, description, body, source}. body exact : écrit sans
    // whitespace de bord, le trim de load_skill n'est pas visible ; source =
    // "workspace" (le skill vit sous <ws>/.vanyline/skills/).
    let resp = c.call("config/skills/get", json!({ "name": "sk-get" }));
    assert_eq!(
        Client::vnl_code(&resp),
        None,
        "skills/get should succeed, got: {resp}"
    );
    let got = &resp["result"];
    assert_eq!(
        got["name"], "sk-get",
        "result should carry the name, got: {resp}"
    );
    assert_eq!(
        got["description"], "d",
        "result should carry the description, got: {resp}"
    );
    assert_eq!(
        got["body"], "ligne 1\nligne 2",
        "result should carry the untrimmed-content body verbatim, got: {resp}"
    );
    assert_eq!(
        got["source"], "workspace",
        "workspace-layer skill should report source:workspace, got: {resp}"
    );

    // Non-régression : le body reste absent de l'index léger config/skills.
    let entry = find_entry(&mut c, "config/skills", "sk-get");
    assert!(
        entry.get("body").is_none(),
        "config/skills index must not carry the body key, entry: {entry}"
    );

    c.shutdown();
}

#[test]
fn smoke_skills_get_unknown_name_returns_006() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // Config vide : UnknownReference côté store -> VNL-RPC-006 (message
    // porteur du VNL-CFG-*), comme les 6 lectures config — VNL-RPC-012 reste
    // réservé aux écritures.
    let resp = c.call("config/skills/get", json!({ "name": "absent" }));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-006"),
        "skills/get on an unknown name should be VNL-RPC-006, got: {resp}"
    );

    c.shutdown();
}

#[test]
fn smoke_skills_get_missing_name_returns_000() {
    let home = tempdir().unwrap();
    let ws = tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".vanyline")).unwrap();
    let mut c = Client::spawn(home.path(), Some(ws.path()));

    // `name` requis : params {} -> échec de désérialisation de l'enveloppe
    // -> VNL-RPC-000, comme les bras d'écriture.
    let resp = c.call("config/skills/get", json!({}));
    assert_eq!(
        Client::vnl_code(&resp).as_deref(),
        Some("VNL-RPC-000"),
        "skills/get without name should be VNL-RPC-000, got: {resp}"
    );

    c.shutdown();
}
