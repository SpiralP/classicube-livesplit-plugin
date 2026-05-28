#![cfg(windows)]

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    sync::broadcast,
    time::{MissedTickBehavior, interval, sleep},
};
use tracing::{debug, info, trace, warn};

use crate::{chat_print_async, plugin::livesplit::protocol::Command};

pub const PIPE_NAME: &str = r"\\.\pipe\LiveSplit";
const BACKOFF_INITIAL: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

pub async fn run(mut command_rx: broadcast::Receiver<Command>, connected: Arc<AtomicBool>) {
    let mut backoff = BACKOFF_INITIAL;
    loop {
        info!("dialing LiveSplit at {PIPE_NAME}");
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(pipe) => {
                info!("connected to LiveSplit at {PIPE_NAME}");
                chat_print_async(format!("&aLiveSplit: connected to {PIPE_NAME}"));
                connected.store(true, Ordering::Relaxed);
                backoff = BACKOFF_INITIAL;

                run_session(pipe, &mut command_rx).await;

                connected.store(false, Ordering::Relaxed);
                info!("disconnected from LiveSplit at {PIPE_NAME}; will reconnect");
                chat_print_async(format!("&eLiveSplit: disconnected from {PIPE_NAME}"));
            }
            Err(e) => {
                debug!("dial failed for {PIPE_NAME}: {e}");
            }
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn run_session(pipe: NamedPipeClient, command_rx: &mut broadcast::Receiver<Command>) {
    let (read_half, mut write_half) = tokio::io::split(pipe);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let mut ping = interval(Duration::from_secs(10));
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Ok(cmd) => {
                        if send_command(&mut write_half, &cmd).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("client lagged; dropped {n} commands");
                    }
                }
            }

            res = reader.read_line(&mut line) => {
                match res {
                    Ok(0) => {
                        debug!("LiveSplit pipe EOF");
                        return;
                    }
                    Ok(_) => {
                        trace!(text = %line.trim_end(), "ls inbound");
                        line.clear();
                    }
                    Err(e) => {
                        debug!("pipe read error: {e}");
                        return;
                    }
                }
            }

            _ = ping.tick() => {
                if send_command(&mut write_half, &Command::Ping).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn send_command<W>(write_half: &mut W, cmd: &Command) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let Some(line) = cmd.to_line() else {
        trace!(?cmd, "ls outbound dropped (no desktop equivalent)");
        return Ok(());
    };
    trace!(?cmd, %line, "ls outbound");
    write_half.write_all(line.as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await
}
