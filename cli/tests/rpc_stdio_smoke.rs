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
