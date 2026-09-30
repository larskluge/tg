use std::io::ErrorKind;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tdlib_rs::enums::Update;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinSet;

use crate::client::{TdLibClient, TelegramClient};
use crate::error::{Result, TgError};
use crate::serve::{self, RequestEnvelope, ResponseEnvelope, StreamEvent};

/// Maximum time to wait for in-flight per-connection tasks to drain after the
/// listener stops accepting new connections.
const SHUTDOWN_DRAIN_SECS: u64 = 5;

/// Maximum byte length of a single NDJSON request line. Any peer that can
/// open the socket can send arbitrary input, so this cap is what stops a
/// malicious or buggy client from OOM-ing the long-lived daemon.
const MAX_REQUEST_LINE_BYTES: usize = 1_048_576; // 1 MiB

/// What a connection needs to serve `subscribe`, besides the client it must
/// not touch: the channel TDLib's updates are broadcast on, and the switch that
/// ends every subscription at shutdown.
///
/// The update channel is taken from the client BEFORE the client goes behind
/// its lock, which is what lets a subscription read updates while a slow
/// `download` or `sync` holds the lock for minutes.
#[derive(Clone)]
pub struct Subscriptions {
    updates: broadcast::Sender<Update>,
    shutdown: watch::Receiver<bool>,
}

impl Subscriptions {
    pub fn new(updates: broadcast::Sender<Update>, shutdown: watch::Receiver<bool>) -> Self {
        Self { updates, shutdown }
    }
}

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

    // A subscription never ends on its own, so shutdown has to end it: without
    // this switch every live subscriber would hold the drain below for its full
    // timeout, eating into the stop budget TDLib's own close needs.
    let (stop_subscriptions, shutdown) = watch::channel(false);
    let subscriptions = Subscriptions::new(client.updates(), shutdown);
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
                        let subscriptions = subscriptions.clone();
                        tasks.spawn(async move {
                            if let Err(e) = handle_connection(stream, client_arc, subscriptions).await {
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

    // Stop accepting, end every subscription, and drain.
    drop(listener);
    stop_subscriptions.send_replace(true);
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
        // Only consider unlinking if the path is actually a Unix socket. A
        // regular file at the socket path is somebody else's data and must
        // not be removed — refuse to start instead. This also normalises
        // platform differences: on Linux, connect() to a non-socket can
        // return ECONNREFUSED, which would otherwise trick us into deleting
        // unrelated files.
        let meta = std::fs::metadata(path)?;
        if !meta.file_type().is_socket() {
            return Err(TgError::Other(format!(
                "tg serve: path {} exists and is not a socket; refusing to start",
                path.display()
            )));
        }

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
                // Real socket with no listener — safe to remove.
                std::fs::remove_file(path)?;
            }
            Ok(Err(e)) => {
                return Err(TgError::Other(format!(
                    "tg serve: socket at {} exists but connect failed unexpectedly ({e}); refusing to start",
                    path.display()
                )));
            }
            Err(_) => {
                return Err(TgError::Other(format!(
                    "tg serve: socket at {} exists and connect timed out; another server may be running. \
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
///
/// A `subscribe` request turns the rest of the connection into an event
/// stream (see [`run_subscription`]); it is answered here, never by the
/// dispatcher, and never takes the client lock.
pub async fn handle_connection<C: TelegramClient>(
    stream: UnixStream,
    client: Arc<Mutex<C>>,
    subscriptions: Subscriptions,
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
            Ok(req) if req.cmd == serve::SUBSCRIBE_CMD => {
                // Attach to the channel BEFORE the ack, so every update after
                // the ack reaches this subscriber.
                let updates = subscriptions.updates.subscribe();
                write_response(&mut write, &serve::subscribe_ack(req.id)).await?;
                return run_subscription(
                    updates,
                    &mut reader,
                    &mut write,
                    serve::HEARTBEAT_INTERVAL,
                    subscriptions.shutdown,
                )
                .await;
            }
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

/// Carry TDLib updates to one subscriber as [`StreamEvent`] frames until the
/// subscriber closes its end (`Ok`), a write to it fails (`Err`), or serve
/// shuts down (`Ok`).
///
/// - Every update that maps to an event is written as one line, flushed.
/// - A receiver that fell behind the broadcast channel writes one `lagged`
///   frame with the number of updates it lost, then carries on.
/// - A `heartbeat` frame goes out every `heartbeat`, starting one interval
///   after the ack. Its only job is to fail when the peer is gone.
/// - Anything the peer sends after `subscribe` is read and discarded: reading
///   is how its close is noticed, and a subscribed connection takes no further
///   requests.
///
/// It holds no client lock at any point; `updates` is all it reads.
async fn run_subscription<R, W>(
    mut updates: broadcast::Receiver<Update>,
    peer: &mut R,
    write: &mut W,
    heartbeat: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut discard = [0u8; 1024];

    loop {
        let event = tokio::select! {
            received = updates.recv() => match received {
                Ok(update) => match StreamEvent::from_update(&update) {
                    Some(event) => event,
                    None => continue,
                },
                Err(RecvError::Lagged(skipped)) => StreamEvent::Lagged { skipped },
                Err(RecvError::Closed) => {
                    return Err(TgError::Other(
                        "subscription ended: the TDLib update channel closed".to_string(),
                    ));
                }
            },
            _ = ticks.tick() => StreamEvent::Heartbeat {},
            read = peer.read(&mut discard) => match read {
                Ok(0) => return Ok(()),
                Ok(_) => continue,
                Err(e) => {
                    return Err(TgError::Other(format!(
                        "subscription ended: reading from the subscriber failed: {e}"
                    )));
                }
            },
            // Also resolves if the switch itself is gone, which only happens
            // when serve is going away anyway.
            _ = shutdown.wait_for(|stop| *stop) => return Ok(()),
        };

        let line = event.to_line()?;
        let written = async {
            write.write_all(line.as_bytes()).await?;
            write.flush().await
        };
        if let Err(e) = written.await {
            return Err(TgError::Other(format!(
                "subscription ended: could not write {}: {e}",
                line.trim_end()
            )));
        }
    }
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

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let client = Arc::new(Mutex::new(MockClient::default()));
        let (subscriptions, _updates, _stop) = subscriptions();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(stream, client, subscriptions)
                .await
                .unwrap();
        });

        let stream = UnixStream::connect(&path).await.unwrap();
        let (read, mut write) = stream.into_split();

        // Read the single error-response line on a task — read_line returns
        // as soon as one `\n` arrives, so we don't depend on EOF timing.
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                reader.read_line(&mut line),
            )
            .await;
            line
        });

        // Push 2 MiB of data without a newline — more than the 1 MiB cap.
        // Write errors are expected once the server closes the connection.
        let chunk = vec![b'A'; 64 * 1024];
        for _ in 0..32 {
            if write.write_all(&chunk).await.is_err() {
                break;
            }
        }
        drop(write); // signal end of write side

        let line = reader.await.unwrap();
        assert!(
            line.contains("exceeds") && line.contains("byte limit"),
            "expected oversized-line error response, got: {line:?}"
        );

        server.await.unwrap();
    }

    /// A connection's view of subscriptions, plus the two handles a test drives
    /// it with: the update channel TDLib would feed, and the shutdown switch.
    fn subscriptions() -> (
        Subscriptions,
        broadcast::Sender<Update>,
        watch::Sender<bool>,
    ) {
        let (updates, _) = broadcast::channel(16);
        let (stop, shutdown) = watch::channel(false);
        (Subscriptions::new(updates.clone(), shutdown), updates, stop)
    }

    fn edited(chat_id: i64, message_id: i64) -> Update {
        Update::MessageEdited(tdlib_rs::types::UpdateMessageEdited {
            chat_id,
            message_id,
            ..Default::default()
        })
    }

    fn unrelated() -> Update {
        Update::ConnectionState(tdlib_rs::types::UpdateConnectionState {
            state: tdlib_rs::enums::ConnectionState::Ready,
        })
    }

    fn edited_frame(chat_id: i64, message_id: i64) -> serde_json::Value {
        serde_json::json!({
            "event": "message_edited",
            "data": {"chat_id": chat_id, "message_id": message_id}
        })
    }

    /// Read the next line the subscriber receives, failing the test rather
    /// than hanging when none arrives.
    async fn next_frame<R: AsyncBufRead + Unpin>(lines: &mut R) -> serde_json::Value {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(5), lines.read_line(&mut line))
            .await
            .expect("a frame within 5s")
            .unwrap();
        assert!(n > 0, "the stream closed before a frame arrived");
        serde_json::from_str(&line).unwrap()
    }

    const NO_HEARTBEAT: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn a_subscription_forwards_events_and_skips_other_updates() {
        let (tx, rx) = broadcast::channel(16);
        let (_stop, shutdown) = watch::channel(false);
        let (server, client) = tokio::io::duplex(4096);
        let sub = tokio::spawn(async move {
            let (mut peer, mut write) = tokio::io::split(server);
            run_subscription(rx, &mut peer, &mut write, NO_HEARTBEAT, shutdown).await
        });

        tx.send(unrelated()).unwrap();
        tx.send(edited(1, 2 << 20)).unwrap();
        tx.send(edited(1, 3 << 20)).unwrap();
        let mut lines = BufReader::new(client);
        assert_eq!(next_frame(&mut lines).await, edited_frame(1, 2 << 20));
        assert_eq!(next_frame(&mut lines).await, edited_frame(1, 3 << 20));

        // The peer closes: the subscription ends, cleanly.
        drop(lines);
        tokio::time::timeout(Duration::from_secs(5), sub)
            .await
            .expect("the subscription ends when the peer closes")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_subscriber_that_fell_behind_is_told_how_much_it_missed() {
        let (tx, rx) = broadcast::channel(2);
        for id in 1..=5 {
            tx.send(edited(1, id)).unwrap();
        }
        let (_stop, shutdown) = watch::channel(false);
        let (server, client) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let (mut peer, mut write) = tokio::io::split(server);
            run_subscription(rx, &mut peer, &mut write, NO_HEARTBEAT, shutdown).await
        });

        let mut lines = BufReader::new(client);
        assert_eq!(
            next_frame(&mut lines).await,
            serde_json::json!({"event": "lagged", "data": {"skipped": 3}})
        );
        // What the channel still held follows the lag report.
        assert_eq!(next_frame(&mut lines).await, edited_frame(1, 4));
        assert_eq!(next_frame(&mut lines).await, edited_frame(1, 5));
    }

    #[tokio::test]
    async fn a_quiet_subscription_writes_heartbeats() {
        let (_tx, rx) = broadcast::channel(16);
        let (_stop, shutdown) = watch::channel(false);
        let (server, client) = tokio::io::duplex(4096);
        let every = Duration::from_millis(50);
        let started = tokio::time::Instant::now();
        tokio::spawn(async move {
            let (mut peer, mut write) = tokio::io::split(server);
            run_subscription(rx, &mut peer, &mut write, every, shutdown).await
        });

        let mut lines = BufReader::new(client);
        let heartbeat = serde_json::json!({"event": "heartbeat", "data": {}});
        assert_eq!(next_frame(&mut lines).await, heartbeat);
        // The first one waits a full interval: the ack already proved the
        // connection alive.
        assert!(started.elapsed() >= every);
        assert_eq!(next_frame(&mut lines).await, heartbeat);
    }

    /// A subscriber whose socket accepts nothing: every write fails as it would
    /// once the peer is gone.
    struct BrokenPipe;

    impl tokio::io::AsyncWrite for BrokenPipe {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::from(ErrorKind::BrokenPipe)))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_failed_heartbeat_write_ends_the_subscription() {
        // The update channel stays open and the peer never closes its end, so
        // the only thing that can end this subscription is the heartbeat.
        let (_tx, rx) = broadcast::channel(16);
        let (_stop, shutdown) = watch::channel(false);
        let (server, _peer_still_open) = tokio::io::duplex(64);
        let (mut peer, _) = tokio::io::split(server);

        let err = tokio::time::timeout(
            Duration::from_secs(5),
            run_subscription(
                rx,
                &mut peer,
                &mut BrokenPipe,
                Duration::from_millis(20),
                shutdown,
            ),
        )
        .await
        .expect("a failed heartbeat ends the subscription")
        .unwrap_err()
        .to_string();
        assert!(err.contains("heartbeat"), "{err}");
    }

    #[tokio::test]
    async fn shutdown_ends_a_subscription() {
        let (_tx, rx) = broadcast::channel(16);
        let (stop, shutdown) = watch::channel(false);
        let (server, _peer_still_open) = tokio::io::duplex(64);
        let sub = tokio::spawn(async move {
            let (mut peer, mut write) = tokio::io::split(server);
            run_subscription(rx, &mut peer, &mut write, NO_HEARTBEAT, shutdown).await
        });

        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), sub)
            .await
            .expect("shutdown ends the subscription")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn subscribe_acks_then_streams_events_without_the_client_lock() {
        use crate::client::mock::MockClient;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let client = Arc::new(Mutex::new(MockClient::default()));
        // Held for the whole test: a subscription that took the lock would
        // never answer, and every read below would time out.
        let _held = client.lock().await;

        let (subscriptions, updates, _stop) = subscriptions();
        let server_client = client.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection(stream, server_client, subscriptions).await
        });

        let stream = UnixStream::connect(&path).await.unwrap();
        let (read, mut write) = stream.into_split();
        write
            .write_all(b"{\"id\":\"1\",\"cmd\":\"subscribe\",\"args\":{}}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(read);
        assert_eq!(
            next_frame(&mut lines).await,
            serde_json::json!({"id": "1", "ok": true, "result": {"subscribed": true}})
        );

        updates.send(unrelated()).unwrap();
        updates.send(edited(-100, 7 << 20)).unwrap();
        assert_eq!(next_frame(&mut lines).await, edited_frame(-100, 7 << 20));

        // Closing the peer's side ends the subscription and the connection.
        drop(write);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the subscription ends when the peer closes")
            .unwrap()
            .unwrap();
    }
}
