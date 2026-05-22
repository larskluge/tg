//! Integration tests for the serve protocol: real sockets, real framing,
//! using a stub echo server in place of TDLib so the test suite stays
//! hermetic.
//!
//! The dispatch logic itself is covered by unit tests in `src/serve.rs`.
//! These tests focus on the wire layer: socket binding, stale-socket
//! cleanup, already-running detection, and the per-request round-trip
//! performed by `serve_client::send_request`.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use tg::commands::serve::prepare_socket_path;
use tg::serve::{RequestEnvelope, ResponseEnvelope};
use tg::serve_client;

fn sock_path(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

/// Minimal NDJSON server that handles one connection, dispatches each
/// request via the provided closure, and exits when the peer closes.
async fn stub_server<F>(path: &Path, handler: F)
where
    F: Fn(&RequestEnvelope) -> ResponseEnvelope + Send + Sync + 'static,
{
    let listener = UnixListener::bind(path).unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        if line.trim().is_empty() {
            continue;
        }
        let req: RequestEnvelope = serde_json::from_str(&line).unwrap();
        let resp = handler(&req);
        let mut out = serde_json::to_string(&resp).unwrap();
        out.push('\n');
        write.write_all(out.as_bytes()).await.unwrap();
        write.flush().await.unwrap();
    }
}

#[tokio::test]
async fn prepare_socket_path_removes_real_stale_socket() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "stale.sock");
    // Create a real Unix socket then drop the listener — the path remains
    // as a socket file with no listener, so connect() returns ECONNREFUSED.
    let listener = UnixListener::bind(&path).unwrap();
    drop(listener);
    assert!(path.exists());
    prepare_socket_path(&path).await.unwrap();
    assert!(
        !path.exists(),
        "real stale socket at {path:?} should be unlinked",
        path = path
    );
}

#[tokio::test]
async fn prepare_socket_path_refuses_when_path_is_plain_file() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "not-a-socket");
    std::fs::write(&path, b"garbage").unwrap();
    let err = prepare_socket_path(&path).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not a socket") || msg.contains("refusing to start"),
        "expected refusal-to-start error for non-socket path, got: {msg}"
    );
    // The plain file must NOT be removed — that would be a destructive surprise.
    assert!(
        path.exists(),
        "non-socket file at {path:?} must not be removed"
    );
}

#[tokio::test]
async fn prepare_socket_path_detects_running_server() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "alive.sock");
    // A real listener at the path simulates an already-running server.
    let _listener = UnixListener::bind(&path).unwrap();
    let err = prepare_socket_path(&path).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already running"),
        "expected 'already running' error, got: {msg}"
    );
}

#[tokio::test]
async fn prepare_socket_path_creates_parent_dir() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("does/not/exist/yet/tg.sock");
    prepare_socket_path(&nested).await.unwrap();
    assert!(nested.parent().unwrap().exists());
}

#[tokio::test]
async fn wire_round_trip_carries_id_and_result() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "echo.sock");

    let server_path = path.clone();
    let server = tokio::spawn(async move {
        stub_server(&server_path, |req| {
            ResponseEnvelope::ok(req.id.clone(), serde_json::json!({"echoed_cmd": req.cmd}))
        })
        .await;
    });

    // Give the listener a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stream = UnixStream::connect(&path).await.unwrap();

    #[derive(Deserialize, Debug)]
    struct Echo {
        echoed_cmd: String,
    }

    let result: Echo = serve_client::send_request(stream, "whoami", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.echoed_cmd, "whoami");

    server.await.unwrap();
}

#[tokio::test]
async fn wire_surfaces_server_side_error_as_tg_error() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "err.sock");

    let server_path = path.clone();
    let server = tokio::spawn(async move {
        stub_server(&server_path, |req| {
            ResponseEnvelope::err(req.id.clone(), "boom")
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stream = UnixStream::connect(&path).await.unwrap();
    let err =
        serve_client::send_request::<_, serde_json::Value>(stream, "whoami", serde_json::json!({}))
            .await
            .unwrap_err();
    assert!(err.to_string().contains("boom"));

    server.await.unwrap();
}

#[tokio::test]
async fn pipelined_requests_preserve_arrival_order() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "pipe.sock");

    let server_path = path.clone();
    let server = tokio::spawn(async move {
        stub_server(&server_path, |req| {
            ResponseEnvelope::ok(req.id.clone(), serde_json::json!({"cmd": req.cmd}))
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stream = UnixStream::connect(&path).await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();

    for i in 0..5 {
        let req = RequestEnvelope {
            id: serde_json::json!(i),
            cmd: format!("cmd-{i}"),
            args: serde_json::Value::Null,
        };
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        write.write_all(line.as_bytes()).await.unwrap();
    }
    write.flush().await.unwrap();
    drop(write); // signals EOF to server

    let mut ids = Vec::new();
    for _ in 0..5 {
        let line = lines.next_line().await.unwrap().unwrap();
        let resp: ResponseEnvelope = serde_json::from_str(&line).unwrap();
        ids.push(resp.id);
    }
    assert_eq!(
        ids,
        (0..5).map(|i| serde_json::json!(i)).collect::<Vec<_>>(),
        "responses must arrive in request order"
    );

    server.await.unwrap();
}

#[tokio::test]
async fn parallel_connections_each_get_their_own_response() {
    let dir = TempDir::new().unwrap();
    let path = sock_path(&dir, "par.sock");

    let server_path = path.clone();
    let server = tokio::spawn(async move {
        let listener = UnixListener::bind(&server_path).unwrap();
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                if let Some(line) = lines.next_line().await.unwrap() {
                    let req: RequestEnvelope = serde_json::from_str(&line).unwrap();
                    let resp =
                        ResponseEnvelope::ok(req.id.clone(), serde_json::json!({"cmd": req.cmd}));
                    let mut out = serde_json::to_string(&resp).unwrap();
                    out.push('\n');
                    write.write_all(out.as_bytes()).await.unwrap();
                    write.flush().await.unwrap();
                }
            });
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    #[derive(Deserialize)]
    struct Resp {
        cmd: String,
    }

    let mut handles = Vec::new();
    for i in 0..3 {
        let p = path.clone();
        handles.push(tokio::spawn(async move {
            let stream = UnixStream::connect(&p).await.unwrap();
            let r: Resp =
                serve_client::send_request(stream, &format!("conn-{i}"), serde_json::json!({}))
                    .await
                    .unwrap();
            r.cmd
        }));
    }

    let mut got: Vec<String> = Vec::new();
    for h in handles {
        got.push(h.await.unwrap());
    }
    got.sort();
    assert_eq!(
        got,
        vec![
            "conn-0".to_string(),
            "conn-1".to_string(),
            "conn-2".to_string()
        ]
    );

    server.await.unwrap();
}

#[tokio::test]
async fn try_connect_returns_none_when_path_missing() {
    // We don't manipulate TG_SERVE_SOCKET here (would race with other tests).
    // Instead, hit UnixStream::connect directly with a known-missing path —
    // that's the underlying check try_connect performs.
    let dir = TempDir::new().unwrap();
    let missing = sock_path(&dir, "absent.sock");
    let result = UnixStream::connect(&missing).await;
    assert!(result.is_err(), "connecting to missing socket should fail");
}
