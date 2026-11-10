//! Optional virus scanning of uploaded and imported files with ClamAV: the file is streamed to
//! `clamd` (INSTREAM) at `WIKICRAFT_CLAMAV` (`tcp://host:3310` or `unix:///run/clamav/clamd.ctl`).
//! Infected files are refused. When the scanner can't be reached (or the file is above clamd's
//! `StreamMaxLength`), uploads are refused too, unless `WIKICRAFT_CLAMAV_FAIL_OPEN` is set.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

const CHUNK: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Clean,
    /// the signature clamd found
    Infected(String),
}

async fn instream<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, data: &[u8]) -> anyhow::Result<Verdict> {
    s.write_all(b"zINSTREAM\0").await?;
    for chunk in data.chunks(CHUNK) {
        s.write_all(&(chunk.len() as u32).to_be_bytes()).await?;
        s.write_all(chunk).await?;
    }
    s.write_all(&0u32.to_be_bytes()).await?;
    s.flush().await?;
    let mut reply = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = s.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        reply.extend_from_slice(&buf[..n]);
        if reply.contains(&0) || reply.len() > 4096 {
            break;
        }
    }
    let reply = String::from_utf8_lossy(&reply).trim_end_matches(['\0', '\n']).to_string();
    // "stream: OK" | "stream: Eicar-Test-Signature FOUND" | "INSTREAM size limit exceeded. ERROR"
    let body = reply.strip_prefix("stream: ").unwrap_or(&reply);
    if body == "OK" {
        Ok(Verdict::Clean)
    } else if let Some(sig) = body.strip_suffix(" FOUND") {
        Ok(Verdict::Infected(sig.trim().to_string()))
    } else {
        anyhow::bail!("clamd: {reply}")
    }
}

/// Scans `data`; `Clean` when no scanner is configured.
pub async fn scan(state: &AppState, data: &[u8]) -> anyhow::Result<Verdict> {
    let Some(target) = state.config.clamav.as_deref().map(str::trim).filter(|t| !t.is_empty()) else { return Ok(Verdict::Clean) };
    let run = async {
        if let Some(addr) = target.strip_prefix("tcp://") {
            instream(tokio::net::TcpStream::connect(addr).await?, data).await
        } else if let Some(path) = target.strip_prefix("unix://") {
            #[cfg(unix)]
            {
                instream(tokio::net::UnixStream::connect(path).await?, data).await
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                anyhow::bail!("unix sockets aren't available on this platform")
            }
        } else {
            anyhow::bail!("WIKICRAFT_CLAMAV must start with tcp:// or unix://")
        }
    };
    tokio::time::timeout(TIMEOUT, run).await.map_err(|_| anyhow::anyhow!("clamd didn't answer in time"))?
}

/// For files being stored: `Ok(None)` to store, `Ok(Some(signature))` for an infected file,
/// and an error when the scanner failed and the wiki doesn't fail open.
pub async fn screen(state: &AppState, data: &[u8]) -> ApiResult<Option<String>> {
    match scan(state, data).await {
        Ok(Verdict::Clean) => Ok(None),
        Ok(Verdict::Infected(sig)) => Ok(Some(sig)),
        Err(e) if state.config.clamav_fail_open => {
            tracing::warn!("virus scan skipped (failing open): {e:#}");
            Ok(None)
        }
        Err(e) => {
            tracing::error!("virus scan failed: {e:#}");
            Err(ApiError::bad("the virus scanner couldn't check this file; try again later or ask an admin"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(reply: &'static [u8], data: &[u8]) -> (anyhow::Result<Verdict>, Vec<u8>) {
        let (client, mut server) = tokio::io::duplex(1 << 20);
        let len = data.len();
        let srv = tokio::spawn(async move {
            let mut got = vec![0u8; 10 + 4 + len + 4];
            server.read_exact(&mut got).await.unwrap();
            server.write_all(reply).await.unwrap();
            got
        });
        let v = instream(client, data).await;
        (v, srv.await.unwrap())
    }

    #[tokio::test]
    async fn speaks_instream() {
        let (v, sent) = ask(b"stream: OK\0", b"hello").await;
        assert_eq!(v.unwrap(), Verdict::Clean);
        assert_eq!(&sent[..10], b"zINSTREAM\0");
        assert_eq!(&sent[10..14], &5u32.to_be_bytes());
        assert_eq!(&sent[14..19], b"hello");
        assert_eq!(&sent[19..], &0u32.to_be_bytes());
        let (v, _) = ask(b"stream: Eicar-Test-Signature FOUND\0", b"x").await;
        assert_eq!(v.unwrap(), Verdict::Infected("Eicar-Test-Signature".into()));
        let (v, _) = ask(b"INSTREAM size limit exceeded. ERROR\0", b"x").await;
        assert!(v.is_err());
    }
}
