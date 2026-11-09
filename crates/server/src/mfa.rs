//! Time-based one-time passwords (RFC 6238: HMAC-SHA1, 30-second steps, 6 digits) and
//! one-time recovery codes for two-factor sign-in.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::AppState;

const STEP: u64 = 30;
const DIGITS: u32 = 6;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = B32.iter().position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// A new 160-bit secret, base32-encoded.
pub fn new_secret() -> String {
    let mut b = [0u8; 20];
    rand::fill(&mut b);
    base32(&b)
}

/// The code for a given 30-second step.
pub fn code_at(secret: &[u8], step: u64) -> String {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[h.len() - 1] & 0x0f) as usize;
    let bin = u32::from_be_bytes([h[off] & 0x7f, h[off + 1], h[off + 2], h[off + 3]]);
    format!("{:0width$}", bin % 10u32.pow(DIGITS), width = DIGITS as usize)
}

pub fn current_step() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / STEP).unwrap_or(0)
}

/// The matching step within ±1 step of now (clock drift), if any.
pub fn matching_step(secret_b32: &str, code: &str) -> Option<u64> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != DIGITS as usize {
        return None;
    }
    let secret = base32_decode(secret_b32)?;
    let now = current_step();
    [now.saturating_sub(1), now, now + 1].into_iter().find(|&s| constant_eq(code_at(&secret, s).as_bytes(), code.as_bytes()))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn otpauth_uri(account: &str, secret: &str) -> String {
    let enc = |s: &str| {
        s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.@".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>()
    };
    format!("otpauth://totp/WikiCraft:{}?secret={secret}&issuer=WikiCraft&algorithm=SHA1&digits={DIGITS}&period={STEP}", enc(account))
}

pub fn qr_svg(data: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes())?;
    Ok(code.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build())
}

fn hash_code(code: &str) -> String {
    let normalized: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

/// Ten fresh recovery codes (shown once) and their hashes (stored).
pub fn recovery_codes() -> (Vec<String>, Vec<String>) {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let codes: Vec<String> = (0..10)
        .map(|_| {
            let mut b = [0u8; 12];
            rand::fill(&mut b);
            let s: String = b.iter().map(|x| ALPHABET[*x as usize % ALPHABET.len()] as char).collect();
            format!("{}-{}-{}", &s[0..4], &s[4..8], &s[8..12])
        })
        .collect();
    let hashes = codes.iter().map(|c| hash_code(c)).collect();
    (codes, hashes)
}

/// Checks a TOTP code (once per step) or consumes a recovery code. `true` = second factor ok.
/// Wrong codes count against a per-user limit (10 per 15 minutes) that a correct password
/// doesn't reset, so codes can't be brute-forced by starting new logins.
pub async fn verify(state: &AppState, user: Uuid, input: &str) -> anyhow::Result<bool> {
    let key = format!("mfa:{user}");
    if !crate::auth::login_allowed(state, &key).await {
        anyhow::bail!(TooManyAttempts);
    }
    let ok = check(state, user, input).await?;
    if ok {
        crate::auth::clear_login_failures(state, &key).await;
    } else {
        crate::auth::record_login_failure(state, &key).await;
    }
    Ok(ok)
}

/// Error for a locked second factor.
#[derive(Debug, thiserror::Error)]
#[error("too many wrong codes; try again in 15 minutes")]
pub struct TooManyAttempts;

async fn check(state: &AppState, user: Uuid, input: &str) -> anyhow::Result<bool> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT totp_secret FROM users WHERE id = $1 AND totp_enabled_at IS NOT NULL").bind(user).fetch_optional(&state.db).await?;
    let Some((Some(secret),)) = row else { return Ok(false) };
    if let Some(step) = matching_step(&secret, input) {
        // atomically refuse a code whose step was already used
        let n = sqlx::query("UPDATE users SET totp_last_step = $2 WHERE id = $1 AND (totp_last_step IS NULL OR totp_last_step < $2)")
            .bind(user)
            .bind(step as i64)
            .execute(&state.db)
            .await?
            .rows_affected();
        return Ok(n == 1);
    }
    let n = sqlx::query("UPDATE users SET recovery_codes = array_remove(recovery_codes, $2) WHERE id = $1 AND $2 = ANY(recovery_codes)")
        .bind(user)
        .bind(hash_code(input))
        .execute(&state.db)
        .await?
        .rows_affected();
    Ok(n == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        // RFC 6238 appendix B, SHA1 secret "12345678901234567890" (8 digits there; last 6 here)
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 59 / 30), "287082");
        assert_eq!(code_at(secret, 1111111109 / 30), "081804");
        assert_eq!(code_at(secret, 2000000000 / 30), "279037");
    }

    #[test]
    fn base32_roundtrip() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("MZXW6YTBOI").unwrap(), b"foobar");
        let s = new_secret();
        assert_eq!(s.len(), 32);
        assert_eq!(base32_decode(&s).unwrap().len(), 20);
    }

    #[test]
    fn current_code_matches() {
        let s = new_secret();
        let code = code_at(&base32_decode(&s).unwrap(), current_step());
        assert!(matching_step(&s, &code).is_some());
        assert!(matching_step(&s, "000000").is_none() || code == "000000");
        assert!(matching_step(&s, "12345").is_none());
        let (codes, hashes) = recovery_codes();
        assert_eq!(codes.len(), 10);
        assert_eq!(hash_code(&codes[0].to_uppercase()), hashes[0], "case and dashes don't matter");
    }
}
