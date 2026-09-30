//! Client-side proxy: try to forward a request to a running `tg serve`, fall
//! back to in-process TDLib when no server is reachable. `tg stream` is the
//! one command with no fallback (see [`stream`]).

use std::io::ErrorKind;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::{Result, TgError};
use crate::serve::{self, RequestEnvelope, ResponseEnvelope};

/// Maximum time the client waits to establish a socket connection. Connect is
/// a kernel-level handshake; this is not a request timeout.
const CONNECT_TIMEOUT_MS: u64 = 500;

/// Connect to a running `tg serve`, or say why there is none: the env var
/// disables it, the socket is missing, or the connect attempt fails.
pub async fn connect() -> Result<UnixStream> {
    let path = serve::socket_path().ok_or_else(|| {
        TgError::Other("tg serve is disabled: TG_SERVE_SOCKET is set but empty".to_string())
    })?;
    match tokio::time::timeout(
        std::time::Duration::from_millis(CONNECT_TIMEOUT_MS),
        UnixStream::connect(&path),
    )
    .await
    {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(TgError::Other(format!(
            "tg serve is not reachable at {}: {e}",
            path.display()
        ))),
        Err(_) => Err(TgError::Other(format!(
            "tg serve is not reachable at {}: connect timed out after {CONNECT_TIMEOUT_MS}ms",
            path.display()
        ))),
    }
}

/// Try to connect to a running `tg serve`. Returns `None` if the socket is
/// missing, the env var disables it, or the connect attempt fails — each of
/// which means "fall back to in-process TDLib", by design.
pub async fn try_connect() -> Option<UnixStream> {
    connect().await.ok()
}

/// Returns `true` when a `tg serve` is reachable on its socket.
pub async fn is_running() -> bool {
    try_connect().await.is_some()
}

/// Send a single request over an established stream and parse the response
/// into `T`. The stream is consumed: this is one request per connection.
pub async fn send_request<A: Serialize, T: DeserializeOwned>(
    stream: UnixStream,
    cmd: &str,
    args: A,
) -> Result<T> {
    let env = RequestEnvelope {
        id: serde_json::Value::String("1".to_string()),
        cmd: cmd.to_string(),
        args: serde_json::to_value(args)
            .map_err(|e| TgError::Other(format!("serialize request: {e}")))?,
    };

    let (read, mut write) = stream.into_split();

    let mut line = serde_json::to_string(&env)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    write.flush().await?;

    let mut lines = BufReader::new(read).lines();
    let response_line = lines.next_line().await?.ok_or_else(|| {
        TgError::Other("tg serve: connection closed without response".to_string())
    })?;

    let response: ResponseEnvelope = serde_json::from_str(&response_line)
        .map_err(|e| TgError::Other(format!("tg serve: invalid response: {e}")))?;

    if response.ok {
        let value = response.result.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value)
            .map_err(|e| TgError::Other(format!("tg serve: result parse error: {e}")))
    } else {
        // A failure's `result` — the per-element record a partial media send
        // carries — is deliberately dropped here: this is the CLI's own proxy,
        // and the CLI has no attachment flag (`SendRequest::from(SendArgs)`
        // never sets `files`), so it cannot receive one. A caller that sends
        // attachments speaks the socket protocol directly and reads both keys.
        Err(TgError::Other(response.error.unwrap_or_else(|| {
            "tg serve: unknown server error".to_string()
        })))
    }
}

/// Why a `tg stream` ended without an error: its consumer is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    /// A write to stdout failed with `BrokenPipe`.
    StdoutClosed,
    /// Stdin reached EOF.
    StdinClosed,
}

/// `tg stream`: subscribe over an established connection and copy every event
/// frame, as received, to `out` — one line each, flushed. The ack is not
/// copied, so `out` carries nothing but `{"event": ...}` lines.
///
/// - `Err` if serve refuses the subscription, sends something that is not an
///   event frame, or closes the stream. A stream that ends is a failure the
///   caller must hear about: nothing tells it about updates any more.
/// - `Ok` once the consumer is gone, which is the one ordinary way for it to
///   end. Either `out` is closed — the next frame finds that out, at the
///   latest serve's heartbeat — or `consumer_gone` resolves (`tg stream`
///   passes stdin's EOF). Both are needed: under `podman exec -i` a consumer
///   that dies never closes the process's stdout, because conmon keeps
///   accepting the writes, but podman does close its stdin.
///
/// Socket-only on purpose: an in-process fallback would open a second TDLib
/// client on the database `tg serve` holds.
pub async fn stream<W, F>(stream: UnixStream, out: &mut W, consumer_gone: F) -> Result<StreamEnd>
where
    W: AsyncWrite + Unpin,
    F: Future<Output = ()>,
{
    let env = RequestEnvelope {
        id: serde_json::Value::String("1".to_string()),
        cmd: serve::SUBSCRIBE_CMD.to_string(),
        args: serde_json::json!({}),
    };

    // The write half stays in scope until this returns: dropping it shuts the
    // socket down for writing, which serve reads as the subscriber leaving.
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_string(&env)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    write.flush().await?;

    let mut lines = BufReader::new(read).lines();
    let ack = lines.next_line().await?.ok_or_else(|| {
        TgError::Other("tg serve: connection closed without response".to_string())
    })?;
    let ack: ResponseEnvelope = serde_json::from_str(&ack)
        .map_err(|e| TgError::Other(format!("tg serve: invalid response: {e}")))?;
    if !ack.ok {
        return Err(TgError::Other(
            ack.error
                .unwrap_or_else(|| "tg serve: unknown server error".to_string()),
        ));
    }
    if ack.result != Some(serde_json::json!({"subscribed": true})) {
        return Err(TgError::Other(format!(
            "tg serve: unexpected reply to subscribe: {}",
            serde_json::to_string(&ack.result).unwrap_or_else(|e| format!("<{e}>"))
        )));
    }

    tokio::pin!(consumer_gone);
    loop {
        let mut frame = tokio::select! {
            next = lines.next_line() => match next? {
                Some(frame) => frame,
                None => {
                    return Err(TgError::Other(
                        "tg stream: tg serve closed the stream".to_string(),
                    ));
                }
            },
            () = &mut consumer_gone => return Ok(StreamEnd::StdinClosed),
        };
        if frame.trim().is_empty() {
            continue;
        }
        if !is_event_frame(&frame) {
            return Err(TgError::Other(format!(
                "tg serve: sent a line that is not an event frame: {frame}"
            )));
        }
        frame.push('\n');
        let written = async {
            out.write_all(frame.as_bytes()).await?;
            out.flush().await
        };
        match written.await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::BrokenPipe => return Ok(StreamEnd::StdoutClosed),
            Err(e) => return Err(TgError::Other(format!("tg stream: writing stdout: {e}"))),
        }
    }
}

/// Resolves once this process's stdin reaches EOF, or can no longer be read.
///
/// It reads on a plain thread that is never joined, not through tokio's
/// `stdin()`: tokio's blocking read cannot be cancelled, so the runtime would
/// wait for it at exit and `tg stream` could not end while stdin stayed open.
/// What is read is discarded.
pub fn stdin_closed() -> impl Future<Output = ()> {
    let (closed, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        if let Err(e) = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink()) {
            eprintln!("tg stream: reading stdin failed ({e}); treating it as closed");
        }
        // Fails only when the stream has already ended and dropped the
        // receiver, and then there is nobody left to tell.
        let _ = closed.send(());
    });
    async move {
        if rx.await.is_err() {
            eprintln!(
                "tg stream: the stdin watcher stopped without an answer; treating stdin as closed"
            );
        }
    }
}

/// `{"event": "<name>", ...}`: the frame shape the protocol reserves for
/// unsolicited messages. The name itself is not checked, so an event a newer
/// serve adds passes through to a consumer that knows it.
fn is_event_frame(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .is_some_and(|v| v.get("event").is_some_and(serde_json::Value::is_string))
}
