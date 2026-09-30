use serde::{Deserialize, Serialize};
use std::{future::Future, io, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{timeout, Instant};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_ROWS: usize = 256;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid enhanced protocol")
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ProcessDisk,
    Temperatures,
    DiskHealth,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Hello { nonce: String },
    Sample { source: Source },
    Heartbeat {},
    Shutdown {},
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub sequence: u64,
    pub body: Command,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u32,
    pub sequence: u64,
    pub body: Response,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Ready {},
    Alive {},
    Stopped {},
    Unavailable {
        reason: Reason,
    },
    Snapshot {
        source: Source,
        observed_at_ms: u64,
        rows: Vec<Reading>,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    NotImplemented,
    WarmingUp,
    PermissionRequired,
    Unsupported,
    Error,
    TimedOut,
    RateLimited,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reading {
    Temperature {
        object: String,
        celsius: f64,
    },
    ProcessDisk {
        pid: u32,
        creation: String,
        read_bps: f64,
        write_bps: f64,
        window_ms: u32,
        complete: bool,
    },
    DiskHealth {
        object: String,
        health: Health,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    Warning,
    Unhealthy,
    Unknown,
}
fn opaque(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
impl Response {
    pub fn validate(&self) -> io::Result<()> {
        if let Self::Snapshot {
            source,
            observed_at_ms,
            rows,
        } = self
        {
            if *observed_at_ms == 0 {
                return Err(invalid());
            }
            if rows.len() > MAX_ROWS {
                return Err(invalid());
            }
            for row in rows {
                let valid = match (source, row) {
                    (Source::Temperatures, Reading::Temperature { object, celsius }) => {
                        opaque(object) && celsius.is_finite() && (-100.0..=250.0).contains(celsius)
                    }
                    (Source::DiskHealth, Reading::DiskHealth { object, .. }) => opaque(object),
                    (
                        Source::ProcessDisk,
                        Reading::ProcessDisk {
                            pid,
                            creation,
                            read_bps,
                            write_bps,
                            window_ms,
                            ..
                        },
                    ) => {
                        *pid > 0
                            && creation.len() <= 20
                            && creation.parse::<u64>().is_ok_and(|v| v > 0)
                            && [read_bps, write_bps]
                                .iter()
                                .all(|v| v.is_finite() && **v >= 0.0)
                            && (100..=60000).contains(window_ms)
                    }
                    _ => false,
                };
                if !valid {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

pub async fn receive<T: for<'de> Deserialize<'de>, R: AsyncRead + Unpin>(
    stream: &mut R,
    deadline: Duration,
) -> io::Result<T> {
    timeout(deadline, async {
        let len = stream.read_u32_le().await? as usize;
        if len == 0 || len > MAX_FRAME {
            return Err(invalid());
        }
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await?;
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "enhanced peer timed out"))?
}
pub async fn send<T: Serialize, W: AsyncWrite + Unpin>(
    stream: &mut W,
    value: &T,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(invalid());
    }
    timeout(Duration::from_secs(2), async {
        stream.write_u32_le(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "enhanced write timed out"))?
}

/// The caller must authenticate the OS pipe peer before calling this function.
/// One request in flight; no unbounded channel. Blocking native sources must be isolated separately.
pub async fn serve<S, F, Fut>(mut stream: S, nonce: &str, mut sample: F) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut(Source) -> Fut,
    Fut: Future<Output = Response>,
{
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let hello: Request = receive(&mut stream, Duration::from_secs(5)).await?;
    if hello.version != VERSION
        || hello.sequence != 0
        || !matches!(hello.body, Command::Hello {nonce: ref n} if n == nonce)
    {
        return Err(invalid());
    }
    send(
        &mut stream,
        &Reply {
            version: VERSION,
            sequence: 0,
            body: Response::Ready {},
        },
    )
    .await?;
    let mut expected = 1u64;
    let mut last_sample: Option<Instant> = None;
    loop {
        let request: Request = receive(&mut stream, Duration::from_secs(5)).await?;
        if request.version != VERSION || request.sequence != expected {
            return Err(invalid());
        }
        expected = expected.checked_add(1).ok_or_else(invalid)?;
        let stop = matches!(request.body, Command::Shutdown {});
        let body = match request.body {
            Command::Heartbeat {} => Response::Alive {},
            Command::Shutdown {} => Response::Stopped {},
            Command::Hello { .. } => return Err(invalid()),
            Command::Sample { source } => {
                if last_sample.is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
                    Response::Unavailable {
                        reason: Reason::RateLimited,
                    }
                } else {
                    last_sample = Some(Instant::now());
                    let value = timeout(Duration::from_secs(2), sample(source))
                        .await
                        .unwrap_or(Response::Unavailable {
                            reason: Reason::TimedOut,
                        });
                    match &value {
                        Response::Snapshot { source: actual, .. } if *actual == source => {}
                        Response::Unavailable { .. } => {}
                        _ => return Err(invalid()),
                    }
                    value.validate()?;
                    value
                }
            }
        };
        send(
            &mut stream,
            &Reply {
                version: VERSION,
                sequence: request.sequence,
                body,
            },
        )
        .await?;
        if stop {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NONCE: &str = "0123456789abcdef0123456789abcdef";
    fn req(sequence: u64, body: Command) -> Request {
        Request {
            version: VERSION,
            sequence,
            body,
        }
    }
    async fn hello(s: &mut tokio::io::DuplexStream) {
        send(
            s,
            &req(
                0,
                Command::Hello {
                    nonce: NONCE.into(),
                },
            ),
        )
        .await
        .unwrap();
        let reply: Reply = receive(s, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(reply.body, Response::Ready {}));
    }
    #[tokio::test]
    async fn rejects_oversized_truncated_unknown_and_wrong_nonce() {
        for bytes in [
            ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec(),
            vec![4, 0, 0, 0, 1],
        ] {
            let (mut a, mut b) = tokio::io::duplex(128);
            a.write_all(&bytes).await.unwrap();
            drop(a);
            assert!(receive::<Request, _>(&mut b, Duration::from_millis(50))
                .await
                .is_err());
        }
        assert!(serde_json::from_str::<Request>(
            r#"{"version":1,"sequence":0,"body":{"command":"execute","path":"cmd.exe"}}"#
        )
        .is_err());
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| async { Response::Alive {} }));
        send(
            &mut a,
            &req(
                0,
                Command::Hello {
                    nonce: "bad".into(),
                },
            ),
        )
        .await
        .unwrap();
        assert!(task.await.unwrap().is_err());
    }
    #[tokio::test]
    async fn synthetic_snapshot_and_orderly_shutdown() {
        let (mut a, b) = tokio::io::duplex(4096);
        let task = tokio::spawn(serve(b, NONCE, |source| async move {
            Response::Snapshot {
                observed_at_ms: 1,
                source,
                rows: vec![Reading::Temperature {
                    object: "synthetic-cpu".into(),
                    celsius: 42.0,
                }],
            }
        }));
        hello(&mut a).await;
        send(
            &mut a,
            &req(
                1,
                Command::Sample {
                    source: Source::Temperatures,
                },
            ),
        )
        .await
        .unwrap();
        let reply: Reply = receive(&mut a, Duration::from_secs(1)).await.unwrap();
        reply.body.validate().unwrap();
        assert!(matches!(reply.body, Response::Snapshot { .. }));
        send(&mut a, &req(2, Command::Shutdown {})).await.unwrap();
        let reply: Reply = receive(&mut a, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(reply.body, Response::Stopped {}));
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn replay_and_disconnect_close_session() {
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| async { Response::Alive {} }));
        hello(&mut a).await;
        send(&mut a, &req(0, Command::Heartbeat {})).await.unwrap();
        assert!(task.await.unwrap().is_err());
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| async { Response::Alive {} }));
        hello(&mut a).await;
        drop(a);
        assert!(task.await.unwrap().is_err());
    }
    #[tokio::test]
    async fn stalled_source_times_out_and_shutdown_still_works() {
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| std::future::pending::<Response>()));
        hello(&mut a).await;
        send(
            &mut a,
            &req(
                1,
                Command::Sample {
                    source: Source::DiskHealth,
                },
            ),
        )
        .await
        .unwrap();
        let reply: Reply = receive(&mut a, Duration::from_secs(3)).await.unwrap();
        assert!(matches!(
            reply.body,
            Response::Unavailable {
                reason: Reason::TimedOut
            }
        ));
        send(&mut a, &req(2, Command::Shutdown {})).await.unwrap();
        let _: Reply = receive(&mut a, Duration::from_secs(1)).await.unwrap();
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn silent_peer_partial_frame_and_slow_reader_are_bounded() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_u32_le(32).await.unwrap();
        assert_eq!(
            receive::<Request, _>(&mut b, Duration::from_millis(30))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let (mut a, _b) = tokio::io::duplex(8);
        assert_eq!(
            send(
                &mut a,
                &req(
                    0,
                    Command::Hello {
                        nonce: NONCE.into()
                    }
                )
            )
            .await
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| async {
            Response::Unavailable {
                reason: Reason::NotImplemented,
            }
        }));
        hello(&mut a).await;
        assert_eq!(
            task.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[tokio::test]
    async fn wrong_version_and_unknown_fields_are_rejected() {
        let (mut a, b) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve(b, NONCE, |_| async { Response::Alive {} }));
        let mut request = req(
            0,
            Command::Hello {
                nonce: NONCE.into(),
            },
        );
        request.version = 2;
        send(&mut a, &request).await.unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(serde_json::from_str::<Request>(
            r#"{"version":1,"sequence":1,"body":{"command":"heartbeat","path":"ignored.exe"}}"#
        )
        .is_err());
    }

    #[test]
    fn rejects_invalid_values_identity_and_row_counts() {
        for value in [f64::NAN, f64::INFINITY, 251.0] {
            assert!(Response::Snapshot {
                observed_at_ms: 1,
                source: Source::Temperatures,
                rows: vec![Reading::Temperature {
                    object: "cpu".into(),
                    celsius: value
                }]
            }
            .validate()
            .is_err());
        }
        assert!(Response::Snapshot {
            observed_at_ms: 1,
            source: Source::DiskHealth,
            rows: (0..=MAX_ROWS)
                .map(|_| Reading::DiskHealth {
                    object: "disk".into(),
                    health: Health::Unknown
                })
                .collect()
        }
        .validate()
        .is_err());
    }
}
