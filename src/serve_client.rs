//! Client-side proxy: try to forward a request to a running `tg serve`, fall
//! back to in-process TDLib when no server is reachable.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::error::{Result, TgError};
use crate::serve::{self, RequestEnvelope, ResponseEnvelope};

/// Maximum time the client waits to establish a socket connection. Connect is
/// a kernel-level handshake; this is not a request timeout.
const CONNECT_TIMEOUT_MS: u64 = 500;

/// Try to connect to a running `tg serve`. Returns `None` if the socket is
/// missing, the env var disables it, or the connect attempt fails.
pub async fn try_connect() -> Option<UnixStream> {
    let path = serve::socket_path()?;
    if !path.exists() {
        return None;
    }
    match tokio::time::timeout(
        std::time::Duration::from_millis(CONNECT_TIMEOUT_MS),
        UnixStream::connect(&path),
    )
    .await
    {
        Ok(Ok(stream)) => Some(stream),
        _ => None,
    }
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
        Err(TgError::Other(response.error.unwrap_or_else(|| {
            "tg serve: unknown server error".to_string()
        })))
    }
}
