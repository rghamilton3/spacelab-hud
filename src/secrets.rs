//! Transparent at-rest encryption for the handful of secret config fields
//! (the Beszel user password and the GitHub PATs).
//!
//! Secrets are held in plaintext in memory but encrypted before they touch
//! disk. The symmetric data key lives in its own `0600` file under the state
//! dir — *separate* from `config.json` — so a leaked config backup (or a stray
//! `git add config.json`) doesn't also hand over the key. ChaCha20-Poly1305
//! (an AEAD) gives both confidentiality and tamper detection.
//!
//! This is proportionate protection for an unattended LAN device: it defends
//! against *casual* exposure (backups, screen-shares, accidental commits), not
//! against an attacker who can already read this process's files — an
//! auto-booting service must be able to decrypt with no human present, so the
//! key necessarily sits where the app can reach it. The key layer is kept
//! deliberately small ([`load_or_create_key`]) so it can later be swapped for a
//! TPM-sealed key without disturbing the call sites in `config.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};

/// Marker prefixed onto an encrypted value as stored in `config.json`. Lets
/// [`is_encrypted`] tell ciphertext apart from a hand-written or
/// pre-encryption plaintext value (which is migrated to ciphertext on the next
/// save). The version segment leaves room to rotate the scheme later.
const ENC_PREFIX: &str = "enc:v1:";

/// ChaCha20-Poly1305 nonce length (96 bits).
const NONCE_LEN: usize = 12;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// ── Key storage ───────────────────────────────────────────────────────

/// Test-only override for the state dir, so the disk round-trip test can point
/// the key file at a temp dir without mutating process-wide env vars (which is
/// unsound across the parallel test harness). A thread-safe `OnceLock` set once
/// before any cipher init.
#[cfg(test)]
pub(crate) static TEST_STATE_DIR: OnceLock<PathBuf> = OnceLock::new();

fn state_dir() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(dir) = TEST_STATE_DIR.get() {
            return dir.clone();
        }
    }
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/state")
        });
    base.join("spacelab-hud")
}

fn key_path() -> PathBuf {
    state_dir().join("secret.key")
}

/// Loads the 32-byte data key, generating and persisting it (mode `0600`) on
/// first use. This is the single seam a future TPM-backed implementation would
/// replace.
fn load_or_create_key() -> Result<[u8; 32]> {
    let path = key_path();

    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
        return Err(anyhow!(
            "secret key at {} is malformed ({} bytes, expected 32) — \
             delete it to regenerate (existing secrets will need re-entry)",
            path.display(),
            bytes.len()
        ));
    }

    // First run: mint a fresh random key and persist it.
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| anyhow!("getrandom failed: {e}"))?;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating state dir {}", dir.display()))?;
        restrict_dir_perms(dir);
    }

    // Create the file with `0600` from the first inode write (via `create_new`
    // + `mode`), closing the window between a default-umask create and a later
    // chmod where the raw key could be world-readable. `create_new` also makes
    // first-run racy-safe: if a concurrent process wins, we fall through to
    // reading its key rather than clobbering it.
    match create_key_file(&path) {
        Ok(mut file) => {
            file.write_all(&key)
                .with_context(|| format!("writing secret key {}", path.display()))?;
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading secret key {}", path.display()))?;
            bytes.as_slice().try_into().map_err(|_| {
                anyhow!(
                    "secret key at {} is malformed ({} bytes, expected 32)",
                    path.display(),
                    bytes.len()
                )
            })
        }
        Err(e) => Err(e).with_context(|| format!("creating secret key {}", path.display())),
    }
}

#[cfg(unix)]
fn create_key_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_key_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn restrict_dir_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
        eprintln!("secrets: failed to chmod 700 {}: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn restrict_dir_perms(_path: &Path) {}

/// Process-wide cipher, lazily initialized from the on-disk key.
fn cipher() -> Result<&'static ChaCha20Poly1305> {
    static CIPHER: OnceLock<ChaCha20Poly1305> = OnceLock::new();
    if let Some(c) = CIPHER.get() {
        return Ok(c);
    }
    // `OnceLock` has no stable fallible initializer, so build the cipher and
    // race to install it — both candidates derive from the same key file, so
    // whichever `set` wins, the result is identical.
    let c = ChaCha20Poly1305::new(Key::from_slice(&load_or_create_key()?));
    let _ = CIPHER.set(c);
    Ok(CIPHER.get().expect("cipher set above"))
}

// ── Public API ────────────────────────────────────────────────────────

/// True if `value` is an encrypted token produced by [`encrypt`].
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(ENC_PREFIX)
}

/// Encrypts `plaintext` into an `enc:v1:<base64>` token (nonce ‖ ciphertext).
pub fn encrypt(plaintext: &str) -> Result<String> {
    encrypt_with(cipher()?, plaintext)
}

/// Decrypts a token produced by [`encrypt`]. Errors if the token is malformed,
/// truncated, or was sealed under a different key.
pub fn decrypt(token: &str) -> Result<String> {
    decrypt_with(cipher()?, token)
}

// ── Cipher-parameterized core (testable without touching the filesystem) ──

fn encrypt_with(cipher: &ChaCha20Poly1305, plaintext: &str) -> Result<String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| anyhow!("getrandom failed: {e}"))?;

    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
        .map_err(|e| anyhow!("encrypt failed: {e}"))?;

    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{ENC_PREFIX}{}", B64.encode(blob)))
}

fn decrypt_with(cipher: &ChaCha20Poly1305, token: &str) -> Result<String> {
    let b64 = token
        .strip_prefix(ENC_PREFIX)
        .ok_or_else(|| anyhow!("value is not an encrypted token"))?;
    let blob = B64.decode(b64).context("base64 decode")?;
    if blob.len() < NONCE_LEN {
        return Err(anyhow!("ciphertext too short"));
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);

    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| anyhow!("decrypt failed (wrong key or tampered data): {e}"))?;
    String::from_utf8(plaintext).context("decrypted value is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cipher() -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(Key::from_slice(&[7u8; 32]))
    }

    #[test]
    fn round_trip_recovers_plaintext() {
        let c = test_cipher();
        let token = encrypt_with(&c, "ghp_supersecret").unwrap();
        assert!(token.starts_with(ENC_PREFIX));
        assert!(!token.contains("ghp_supersecret")); // not stored in the clear
        assert_eq!(decrypt_with(&c, &token).unwrap(), "ghp_supersecret");
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let c = test_cipher();
        // Same plaintext, different ciphertext — confirms the nonce isn't reused.
        assert_ne!(
            encrypt_with(&c, "same").unwrap(),
            encrypt_with(&c, "same").unwrap()
        );
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let token = encrypt_with(&test_cipher(), "secret").unwrap();
        let other = ChaCha20Poly1305::new(Key::from_slice(&[9u8; 32]));
        assert!(decrypt_with(&other, &token).is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let c = test_cipher();
        let token = encrypt_with(&c, "secret").unwrap();
        let mut bad = token.clone();
        // Flip the last base64 char to corrupt the auth tag.
        let last = bad.pop().unwrap();
        bad.push(if last == 'A' { 'B' } else { 'A' });
        assert!(decrypt_with(&c, &bad).is_err());
    }

    #[test]
    fn is_encrypted_distinguishes_tokens() {
        assert!(is_encrypted("enc:v1:abc"));
        assert!(!is_encrypted("ghp_plaintext"));
        assert!(!is_encrypted(""));
    }
}
