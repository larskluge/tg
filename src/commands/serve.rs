use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::client::{TdLibClient, TelegramClient};
use crate::error::{Result, TgError};
use crate::serve::{self, RequestEnvelope, ResponseEnvelope};

/// Maximum time to wait for in-flight per-connection tasks to drain after the
/// listener stops accepting new connections.
const SHUTDOWN_DRAIN_SECS: u64 = 5;

/// Run the long-lived serve loop until SIGTERM/SIGINT or unrecoverable error.
///
/// Takes ownership of `client` and is responsible for shutting it down on exit.
pub async fn run(mut client: TdLibClient) -> Result<()> {
    let path = serve::socket_path().ok_or_else(|| {
        TgError::Other("TG_SERVE_SOCKET is empty; refusing to start tg serve".to_string())
    })?;

    prepare_socket_path(&path).await?;

    // Initialize TDLib once and wait for the post-auth update sync so every
    // request served from here on sees a fully-synced cache.
    client.start().await?;
    client.wait_for_sync().await;

    let listener = UnixListener::bind(&path)?;
    if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!(
            "tg serve: warning: failed to chmod socket {}: {e}",
            path.display()
        );
    }

    eprintln!("tg serve: listening at {}", path.display());

    let client_arc = Arc::new(Mutex::new(client));
    let mut tasks: JoinSet<()> = JoinSet::new();

    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| TgError::Other(format!("install sigterm handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| TgError::Other(format!("install sigint handler: {e}")))?;

    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, _)) => {
                        let client_arc = client_arc.clone();
                        tasks.spawn(async move {
                            if let Err(e) = handle_connection(stream, client_arc).await {
                                eprintln!("tg serve: connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("tg serve: accept error: {e}");
                    }
                }
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }

    // Stop accepting and drain.
    drop(listener);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(SHUTDOWN_DRAIN_SECS), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}

    // Reclaim client and shut down TDLib cleanly.
    let mut client = Arc::try_unwrap(client_arc)
        .map_err(|_| TgError::Other("could not reclaim TDLib client at shutdown".to_string()))?
        .into_inner();
    client.shutdown().await;

    // Best-effort socket cleanup.
    let _ = std::fs::remove_file(&path);

    Ok(())
}

/// Prepare the socket path: detect "already running", remove stale sockets,
/// and ensure the parent directory exists.
pub async fn prepare_socket_path(path: &Path) -> Result<()> {
    if path.exists() {
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            UnixStream::connect(path),
        )
        .await
        {
            Ok(Ok(_)) => {
                return Err(TgError::Other(format!(
                    "tg serve: already running at {}",
                    path.display()
                )));
            }
            _ => {
                // Stale socket — remove it before binding.
                std::fs::remove_file(path)?;
            }
        }
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    Ok(())
}

/// Read newline-delimited request envelopes from `stream`, dispatch each
/// against the shared client (serializing access through the mutex), and
/// write the response back. Returns when the peer closes the connection.
pub async fn handle_connection<C: TelegramClient>(
    stream: UnixStream,
    client: Arc<Mutex<C>>,
) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<RequestEnvelope>(&line) {
            Ok(req) => {
                let guard = client.lock().await;
                serve::dispatch(&*guard, req).await
            }
            Err(e) => {
                let id = extract_id(&line);
                ResponseEnvelope::err(id, format!("invalid request: {e}"))
            }
        };
        let mut out = serde_json::to_string(&response)?;
        out.push('\n');
        write.write_all(out.as_bytes()).await?;
        write.flush().await?;
    }
    Ok(())
}

fn extract_id(line: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_id_from_partial_request() {
        let v = extract_id(r#"{"id":"abc","cmd":"oops""#); // truncated
        assert!(v.is_null());
    }

    #[test]
    fn extract_id_from_well_formed_object() {
        let v = extract_id(r#"{"id":"abc","cmd":42}"#);
        assert_eq!(v, serde_json::json!("abc"));
    }

    #[test]
    fn extract_id_missing_returns_null() {
        let v = extract_id(r#"{"cmd":"whoami"}"#);
        assert!(v.is_null());
    }
}
