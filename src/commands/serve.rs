use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// Maximum byte length of a single NDJSON request line. Any peer that can
/// open the socket can send arbitrary input, so this cap is what stops a
/// malicious or buggy client from OOM-ing the long-lived daemon.
const MAX_REQUEST_LINE_BYTES: usize = 1_048_576; // 1 MiB

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

    // Bind under a restrictive umask so the socket is created with mode 0600
    // atomically — no window where another local user can connect to a
    // world-accessible socket. Restore the previous umask immediately after.
    let listener = bind_with_restricted_umask(&path)?;
    // Defense in depth: explicitly chmod after bind. Failure is fatal —
    // if we can't guarantee 0600 perms, refuse to serve.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        // Clean up before bailing.
        let _ = std::fs::remove_file(&path);
        TgError::Other(format!("failed to chmod socket {}: {e}", path.display()))
    })?;

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

/// Bind a `UnixListener` at `path` with the process umask temporarily set to
/// `0o077`, so the socket file is created mode `0600` from the kernel's
/// perspective with no race window. Restores the prior umask before returning.
fn bind_with_restricted_umask(path: &Path) -> Result<UnixListener> {
    // SAFETY: libc::umask is process-global. This runs during single-threaded
    // server startup before any tasks are spawned, so there is no concurrent
    // filesystem caller whose umask we could trample.
    let prev = unsafe { libc::umask(0o077) };
    let result = UnixListener::bind(path).map_err(TgError::Io);
    unsafe {
        libc::umask(prev);
    }
    result
}

/// Prepare the socket path: detect "already running", remove stale sockets,
/// and ensure the parent directory exists.
///
/// A socket is considered "stale" only when `connect()` returns
/// `ConnectionRefused` (no listener is attached to it). Any other error
/// — timeout, permission denied, ENOTSOCK, etc. — is treated as
/// "possibly running" and causes us to refuse to start. This avoids the
/// dangerous case where a slow-but-alive server's socket gets unlinked
/// because we couldn't connect within the timeout, allowing two `tg serve`
/// processes to compete for the same TDLib database.
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
            Ok(Err(e)) if e.kind() == ErrorKind::ConnectionRefused => {
                // Definitively no listener on a real Unix socket — safe to remove.
                std::fs::remove_file(path)?;
            }
            Ok(Err(e)) => {
                return Err(TgError::Other(format!(
                    "tg serve: path {} exists and is not a stale socket (connect: {e}); refusing to start",
                    path.display()
                )));
            }
            Err(_) => {
                return Err(TgError::Other(format!(
                    "tg serve: path {} exists and connect timed out; another server may be running. \
                     Refusing to remove. If you are sure no server is running, delete the file manually.",
                    path.display()
                )));
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
    let mut reader = BufReader::new(read);

    loop {
        let mut line = String::new();
        match read_line_bounded(&mut reader, &mut line, MAX_REQUEST_LINE_BYTES).await {
            Ok(false) => break, // peer closed
            Ok(true) => {}
            Err(LineError::TooLong) => {
                // Tell the peer what happened, then drop them. Continuing to
                // parse on the same stream is dangerous — we don't know where
                // we are in the request framing.
                let resp = ResponseEnvelope::err(
                    serde_json::Value::Null,
                    format!(
                        "request line exceeds {MAX_REQUEST_LINE_BYTES}-byte limit; connection closed"
                    ),
                );
                let _ = write_response(&mut write, &resp).await;
                break;
            }
            Err(LineError::Io(e)) => return Err(TgError::Io(e)),
        }

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
        write_response(&mut write, &response).await?;
    }
    Ok(())
}

async fn write_response<W: AsyncWriteExt + Unpin>(
    write: &mut W,
    response: &ResponseEnvelope,
) -> Result<()> {
    let mut out = serde_json::to_string(response)?;
    out.push('\n');
    write.write_all(out.as_bytes()).await?;
    write.flush().await?;
    Ok(())
}

#[derive(Debug)]
enum LineError {
    Io(std::io::Error),
    TooLong,
}

impl From<std::io::Error> for LineError {
    fn from(e: std::io::Error) -> Self {
        LineError::Io(e)
    }
}

/// Read one `\n`-terminated line into `out`, capping at `max` bytes. Returns
/// `Ok(true)` if a line was read (with trailing `\r?\n` stripped), `Ok(false)`
/// on EOF before any bytes, and `Err(TooLong)` if the line exceeded the cap.
/// On `TooLong`, the over-budget bytes have been consumed from the reader.
async fn read_line_bounded<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut String,
    max: usize,
) -> std::result::Result<bool, LineError> {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF
            if buf.is_empty() {
                return Ok(false);
            } else {
                // Partial line at EOF — return what we have.
                push_decoded(&buf, out)?;
                return Ok(true);
            }
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(nl_pos) => {
                let take = nl_pos + 1;
                if buf.len() + take > max + 1 {
                    reader.consume(take);
                    return Err(LineError::TooLong);
                }
                buf.extend_from_slice(&available[..take]);
                reader.consume(take);
                // strip \r?\n
                if buf.ends_with(b"\n") {
                    buf.pop();
                }
                if buf.ends_with(b"\r") {
                    buf.pop();
                }
                push_decoded(&buf, out)?;
                return Ok(true);
            }
            None => {
                let n = available.len();
                if buf.len() + n > max {
                    reader.consume(n);
                    return Err(LineError::TooLong);
                }
                buf.extend_from_slice(available);
                reader.consume(n);
            }
        }
    }
}

fn push_decoded(buf: &[u8], out: &mut String) -> std::result::Result<(), LineError> {
    let s = std::str::from_utf8(buf).map_err(|_| {
        LineError::Io(std::io::Error::new(
            ErrorKind::InvalidData,
            "request line is not valid UTF-8",
        ))
    })?;
    out.push_str(s);
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

    #[tokio::test]
    async fn read_line_bounded_returns_short_lines() {
        let data = b"hello\nworld\n";
        let mut reader = tokio::io::BufReader::new(&data[..]);
        let mut s = String::new();
        assert!(read_line_bounded(&mut reader, &mut s, 64).await.unwrap());
        assert_eq!(s, "hello");
        s.clear();
        assert!(read_line_bounded(&mut reader, &mut s, 64).await.unwrap());
        assert_eq!(s, "world");
        s.clear();
        assert!(!read_line_bounded(&mut reader, &mut s, 64).await.unwrap());
    }

    #[tokio::test]
    async fn read_line_bounded_rejects_oversized_line() {
        let mut data = vec![b'A'; 1024];
        data.push(b'\n');
        let mut reader = tokio::io::BufReader::new(&data[..]);
        let mut s = String::new();
        let err = read_line_bounded(&mut reader, &mut s, 128)
            .await
            .unwrap_err();
        assert!(matches!(err, LineError::TooLong));
    }

    #[tokio::test]
    async fn read_line_bounded_handles_partial_line_at_eof() {
        let data = b"no-newline";
        let mut reader = tokio::io::BufReader::new(&data[..]);
        let mut s = String::new();
        assert!(read_line_bounded(&mut reader, &mut s, 64).await.unwrap());
        assert_eq!(s, "no-newline");
    }

    #[tokio::test]
    async fn read_line_bounded_strips_crlf() {
        let data = b"hello\r\n";
        let mut reader = tokio::io::BufReader::new(&data[..]);
        let mut s = String::new();
        assert!(read_line_bounded(&mut reader, &mut s, 64).await.unwrap());
        assert_eq!(s, "hello");
    }

    #[tokio::test]
    async fn handle_connection_rejects_oversized_line_and_closes() {
        use crate::client::mock::MockClient;
        use tempfile::TempDir;
        use tokio::io::AsyncReadExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let client = Arc::new(Mutex::new(MockClient::default()));
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(stream, client).await.unwrap();
        });

        // Give the listener a moment to be ready.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let stream = UnixStream::connect(&path).await.unwrap();
        let (read, mut write) = stream.into_split();

        // Drive writes on a separate task so we can read the server's error
        // response in parallel. The server is expected to close the
        // connection mid-write once it sees too many bytes without a
        // newline, so write errors here are expected and ignored.
        let writer = tokio::spawn(async move {
            let chunk = vec![b'A'; 64 * 1024];
            // Up to 32 MiB; the server should kill us long before this.
            for _ in 0..512 {
                if write.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        });

        // Read what the server sends before it closes.
        let mut response = String::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::io::AsyncReadExt::read_to_string(
                &mut tokio::io::BufReader::new(read),
                &mut response,
            ),
        )
        .await;

        let _ = writer.await;

        assert!(
            response.contains("exceeds") && response.contains("byte limit"),
            "expected oversized-line error response, got: {response:?}"
        );

        server.await.unwrap();
    }
}
