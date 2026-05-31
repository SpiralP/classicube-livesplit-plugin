#[cfg(test)]
mod tests;

use std::{
    future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use classicube_helpers::async_manager;
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::Deserialize;
use serde_json::Value;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc},
    time::{MissedTickBehavior, interval, timeout},
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite};
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, error, info, trace, warn};
use tungstenite::Message;

use crate::{chat_print_async, plugin::livesplit::protocol::Command};

pub const BIND_ADDR: &str = "127.0.0.1:16833";

/// Max wait for a response after sending a command. Shorter than the 10s
/// ping interval so a stalled client is detected before pings stack up.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;
type WsStream = SplitStream<WebSocketStream<TcpStream>>;

// Drop order is declaration order; `reader_handle` stays first so its
// `AbortOnDropHandle` cancels the reader before the WS write half and
// response channel go.
struct Connection {
    reader_handle: AbortOnDropHandle<()>,
    write: WsSink,
    response_rx: mpsc::UnboundedReceiver<Response>,
}

#[derive(Debug)]
enum Response {
    Success,
    Error {
        code: String,
        message: Option<String>,
    },
}

#[derive(Debug)]
enum SendError {
    Transport(tungstenite::Error),
    Disconnected,
    Timeout,
    LiveSplit {
        code: String,
        message: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ServerResponse {
    Success {
        #[expect(
            dead_code,
            reason = "payload only used for query commands we don't send"
        )]
        success: Value,
    },
    Error {
        error: ServerError,
    },
    Event {
        event: String,
    },
}

#[derive(Debug, Deserialize)]
struct ServerError {
    code: String,
    #[serde(default)]
    message: Option<String>,
}

pub async fn run(mut command_rx: broadcast::Receiver<Command>, connected: Arc<AtomicBool>) {
    let listener = match TcpListener::bind(BIND_ADDR).await {
        Ok(l) => {
            info!("LiveSplit WS server listening on ws://{BIND_ADDR}");
            l
        }
        Err(e) => {
            error!("failed to bind LiveSplit WS server on {BIND_ADDR}: {e}");
            chat_print_async(format!(
                "&cLiveSplit: failed to bind server on {BIND_ADDR}: {e}"
            ));
            return;
        }
    };

    let mut conn: Option<Connection> = None;
    let mut ping = interval(Duration::from_secs(10));
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, peer) = match accept {
                    Ok(p) => p,
                    Err(e) => { warn!("accept() error: {e}"); continue; }
                };
                match accept_async(stream).await {
                    Ok(new_ws) => {
                        if let Some(mut old) = conn.take() {
                            let _ = old.write.close().await;
                            debug!("closed previous LiveSplit client (replaced by {peer})");
                            chat_print_async(
                                "&eLiveSplit: previous server client kicked".to_string(),
                            );
                        }
                        let (write, read) = new_ws.split();
                        let (tx, rx) = mpsc::unbounded_channel();
                        let reader_handle =
                            AbortOnDropHandle::new(async_manager::spawn(read_loop(read, tx)));
                        conn = Some(Connection {
                            reader_handle,
                            write,
                            response_rx: rx,
                        });
                        connected.store(true, Ordering::Relaxed);
                        info!("LiveSplit client connected from {peer}");
                        chat_print_async(format!("&aLiveSplit: server client connected ({peer})"));
                    }
                    Err(e) => warn!("WS handshake failed from {peer}: {e}"),
                }
            }

            cmd = command_rx.recv() => {
                let cmd = match cmd {
                    Ok(cmd) => cmd,
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("command channel closed; LiveSplit server shutting down");
                        if let Some(mut old) = conn.take() {
                            let _ = old.write.close().await;
                        }
                        connected.store(false, Ordering::Relaxed);
                        return;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("server lagged; dropped {n} commands");
                        continue;
                    }
                };
                let result = if let Some(c) = conn.as_mut() {
                    Some(send_and_await(&mut c.write, &mut c.response_rx, &cmd).await)
                } else {
                    None
                };
                if let Some(result) = result {
                    handle_send_result(result, &cmd, &mut conn, &connected);
                }
            }

            _ = wait_reader_exit(&mut conn) => {
                debug!("reader task exited; dropping LiveSplit client");
                drop_conn(&mut conn, &connected);
            }

            _ = ping.tick() => {
                let result = if let Some(c) = conn.as_mut() {
                    Some(send_and_await(&mut c.write, &mut c.response_rx, &Command::Ping).await)
                } else {
                    None
                };
                if let Some(result) = result {
                    handle_send_result(result, &Command::Ping, &mut conn, &connected);
                }
            }
        }
    }
}

/// Reads inbound frames from the WS, forwarding success/error responses to
/// `tx` and logging `{"event":...}` pushes inline at `info!`. Returns when
/// the connection closes; dropping `tx` signals the writer side that no
/// further responses will arrive.
async fn read_loop(mut read: WsStream, tx: mpsc::UnboundedSender<Response>) {
    while let Some(frame) = read.next().await {
        match frame {
            Ok(Message::Text(t)) => {
                trace!(text = %t.as_str(), "ls inbound");
                match serde_json::from_str::<ServerResponse>(t.as_str()) {
                    Ok(ServerResponse::Success { .. }) => {
                        if tx.send(Response::Success).is_err() {
                            return;
                        }
                    }
                    Ok(ServerResponse::Error {
                        error: ServerError { code, message },
                    }) => {
                        if tx.send(Response::Error { code, message }).is_err() {
                            return;
                        }
                    }
                    Ok(ServerResponse::Event { event }) => {
                        info!(%event, "LiveSplit event");
                    }
                    Err(e) => {
                        warn!(error = %e, raw = %t.as_str(), "ls inbound parse failed");
                    }
                }
            }
            Ok(Message::Binary(_)) => warn!("ls inbound binary frame (protocol is JSON text)"),
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Close(c)) => {
                debug!(?c, "LiveSplit client closed");
                return;
            }
            Err(e) => {
                warn!("WS read error: {e}");
                return;
            }
        }
    }
    debug!("LiveSplit client stream ended");
}

/// Sends `cmd` and awaits the matching success/error response. Intentionally
/// blocks the caller's `select!` arm — commands serialize through this — so
/// the wait is capped at `RESPONSE_TIMEOUT` to keep an unresponsive client
/// from wedging accept/ping/reader-exit handling.
async fn send_and_await(
    write: &mut WsSink,
    response_rx: &mut mpsc::UnboundedReceiver<Response>,
    cmd: &Command,
) -> Result<(), SendError> {
    let json = serde_json::to_string(cmd).expect("Command serialization is infallible");
    trace!(?cmd, %json, "ls outbound");
    write
        .send(Message::text(json))
        .await
        .map_err(SendError::Transport)?;
    match timeout(RESPONSE_TIMEOUT, response_rx.recv()).await {
        Ok(Some(Response::Success)) => Ok(()),
        Ok(Some(Response::Error { code, message })) => Err(SendError::LiveSplit { code, message }),
        Ok(None) => Err(SendError::Disconnected),
        Err(_) => Err(SendError::Timeout),
    }
}

fn handle_send_result(
    result: Result<(), SendError>,
    cmd: &Command,
    conn: &mut Option<Connection>,
    connected: &AtomicBool,
) {
    match result {
        Ok(()) => {}
        Err(SendError::LiveSplit { code, message }) => {
            warn!(?cmd, %code, ?message, "LiveSplit rejected command");
            let detail = message
                .as_deref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default();
            chat_print_async(format!("&eLiveSplit error: {code}{detail}"));
        }
        Err(SendError::Transport(e)) => {
            warn!("WS transport error; dropping client: {e}");
            drop_conn(conn, connected);
        }
        Err(SendError::Disconnected) => {
            warn!("LiveSplit client disconnected mid-request");
            drop_conn(conn, connected);
        }
        Err(SendError::Timeout) => {
            warn!(
                ?cmd,
                ?RESPONSE_TIMEOUT,
                "LiveSplit response timed out; dropping client"
            );
            drop_conn(conn, connected);
        }
    }
}

fn drop_conn(conn: &mut Option<Connection>, connected: &AtomicBool) {
    *conn = None;
    connected.store(false, Ordering::Relaxed);
    chat_print_async("&eLiveSplit: server client disconnected".to_string());
}

/// Resolves when the reader task for the current connection exits. Sleeps
/// indefinitely when there is no connection so other `select!` arms run
/// unimpeded.
async fn wait_reader_exit(conn: &mut Option<Connection>) {
    match conn.as_mut() {
        Some(c) => {
            let _ = (&mut c.reader_handle).await;
        }
        None => future::pending().await,
    }
}
