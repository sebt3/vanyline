mod handlers;
mod protocol;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Point d'entrée de `vanyline serve --stdio`. Lit des lignes ndjson sur
/// stdin, dispatch via `handlers::handle_line`, écrit les réponses sur
/// stdout via un canal dédié à un unique writer. Sort (retourne) quand
/// `shutdown` a été traité avec succès, OU quand stdin atteint EOF.
pub async fn run_stdio_server() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdout.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    let mut state = handlers::ServerState::new();
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(response) = handlers::handle_line(&mut state, &line).await {
                    if tx.send(response).is_err() {
                        break;
                    }
                }
                if state.shutdown_requested {
                    break;
                }
            }
            Ok(None) => break, // EOF
            Err(_) => break,
        }
    }

    drop(tx);
    let _ = writer.await;
}