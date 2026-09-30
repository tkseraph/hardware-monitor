//! Explicit, ordinary-user IPC verification. No sensor or ETW collection.
#[cfg(target_os = "windows")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use app_lib::enhanced::{pipe, protocol::*};
    use std::{io::Read, process::Stdio, time::Duration};
    use tokio::io::AsyncWriteExt;
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--client") {
        if args.len() != 4 {
            return Err("invalid test arguments".into());
        }
        let peer = pipe::Peer::open(args[3].parse()?)?;
        let mut nonce = [0u8; 32];
        std::io::stdin().read_exact(&mut nonce)?;
        let nonce = String::from_utf8(nonce.to_vec())?;
        let mut stream = pipe::connect(&args[2], &peer)?;
        for (sequence, body) in [
            Command::Hello { nonce },
            Command::Heartbeat {},
            Command::Sample {
                source: Source::Temperatures,
            },
            Command::Shutdown {},
        ]
        .into_iter()
        .enumerate()
        {
            send(
                &mut stream,
                &Request {
                    version: VERSION,
                    sequence: sequence as u64,
                    body,
                },
            )
            .await?;
            let reply: Reply = receive(&mut stream, Duration::from_secs(3)).await?;
            if reply.version != VERSION || reply.sequence != sequence as u64 {
                return Err("reply identity mismatch".into());
            }
            let valid = match sequence {
                0 => matches!(reply.body, Response::Ready {}),
                1 => matches!(reply.body, Response::Alive {}),
                2 => matches!(
                    reply.body,
                    Response::Unavailable {
                        reason: Reason::NotImplemented
                    }
                ),
                3 => matches!(reply.body, Response::Stopped {}),
                _ => false,
            };
            if !valid {
                return Err("unexpected response".into());
            }
        }
        return Ok(());
    }
    if args.len() != 1 {
        return Err("no options supported".into());
    }
    let (name, server) = pipe::create()?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args(["--client", &name, &std::process::id().to_string()])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let peer = pipe::Peer::open(child.id().ok_or("missing child")?)?;
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(nonce.as_bytes())
        .await?;
    let exchange = async {
        pipe::accept(&server, &peer).await?;
        serve(server, &nonce, |_| async {
            Response::Unavailable {
                reason: Reason::NotImplemented,
            }
        })
        .await
    };
    let result = tokio::time::timeout(Duration::from_secs(12), exchange).await;
    if !matches!(result, Ok(Ok(()))) {
        child.kill().await?;
        child.wait().await?;
        return Err("IPC check failed".into());
    }
    let exit = tokio::time::timeout(Duration::from_secs(3), child.wait()).await??;
    if !exit.success() {
        return Err("client did not exit successfully".into());
    }
    println!("ordinary-user IPC: authenticated child, handshake, heartbeat, unavailable source, shutdown and child exit passed");
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Windows IPC check only");
}
