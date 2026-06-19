use aes_gcm::{AeadInPlace, Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::{Context, Result, bail};
use rand::RngCore;

/// Encrypts plaintext with AES-256-GCM using the app master key.
/// Returns hex-encoded `nonce(12) || ciphertext || tag(16)`.
pub fn encrypt(key: &Aes256Gcm, plaintext: &str) -> Result<String> {
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let mut buf = plaintext.as_bytes().to_vec();
    key.encrypt_in_place(nonce, b"", &mut buf)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&buf);
    Ok(hex::encode(out))
}

/// Decrypts a value produced by `encrypt`.
pub fn decrypt(key: &Aes256Gcm, encoded: &str) -> Result<String> {
    let raw = hex::decode(encoded).context("invalid hex in credentials")?;
    if raw.len() < 12 + 16 {
        bail!("credential blob too short");
    }
    let (nonce_bytes, ciphertext) = raw.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let mut buf = ciphertext.to_vec();
    key.decrypt_in_place(nonce, b"", &mut buf)
        .map_err(|_| anyhow::anyhow!("decryption failed — wrong key or tampered data"))?;

    String::from_utf8(buf).context("decrypted bytes are not valid UTF-8")
}

/// Derive the raw 32-byte AES key from a secret string.
/// Secrets shorter than 32 bytes are zero-padded; longer ones are **truncated to
/// the first 32 bytes** — so only the first 32 bytes of `BIFROST_SECRET` affect
/// the key (a 64-char `openssl rand -hex 32` uses half its entropy, and two
/// secrets sharing a 32-byte prefix collide). Deterministic: the same secret
/// always yields the same key.
fn derive_key_bytes(secret: &str) -> [u8; 32] {
    let mut key_bytes = [0u8; 32];
    let src = secret.as_bytes();
    let len = src.len().min(32);
    key_bytes[..len].copy_from_slice(&src[..len]);
    key_bytes
}

/// Derive an `Aes256Gcm` cipher from a raw secret string.
pub fn cipher_from_secret(secret: &str) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&derive_key_bytes(secret)))
}

/// A short, **non-reversible** fingerprint of the *derived key* (SHA-256 of the
/// key bytes, first 8 hex chars). Logged at startup so an operator can confirm
/// the effective key is stable across restarts: if this fingerprint changes
/// while `BIFROST_SECRET` is believed unchanged, the secret's bytes actually
/// differ at runtime — trailing whitespace/newline, a stale exported env var
/// shadowing `.env` (dotenvy doesn't override an already-set var), or a change
/// only beyond the first 32 bytes — which is the usual cause of "same secret,
/// can't decrypt". Reveals nothing about the secret itself.
pub fn key_fingerprint(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(derive_key_bytes(secret));
    hex::encode(&digest[..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cipher() -> Aes256Gcm {
        cipher_from_secret("test-secret-exactly-32-bytes-xx!")
    }

    #[test]
    fn roundtrip() {
        let cipher = test_cipher();
        let original = "api-key-abc-123";
        let encrypted = encrypt(&cipher, original).unwrap();
        let decrypted = decrypt(&cipher, &encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn empty_string_roundtrip() {
        let cipher = test_cipher();
        let encrypted = encrypt(&cipher, "").unwrap();
        assert_eq!(decrypt(&cipher, &encrypted).unwrap(), "");
    }

    #[test]
    fn each_encryption_produces_unique_ciphertext() {
        let cipher = test_cipher();
        let a = encrypt(&cipher, "same").unwrap();
        let b = encrypt(&cipher, "same").unwrap();
        // Different nonces → different ciphertext even for identical plaintext.
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let cipher_a = cipher_from_secret("key-a-padded-to-32-bytes-xxxxxx!");
        let cipher_b = cipher_from_secret("key-b-padded-to-32-bytes-xxxxxx!");
        let encrypted = encrypt(&cipher_a, "secret").unwrap();
        assert!(decrypt(&cipher_b, &encrypted).is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let cipher = test_cipher();
        let mut encrypted = encrypt(&cipher, "secret").unwrap();
        // Flip the last hex character to corrupt the GCM tag.
        let last = encrypted.pop().unwrap();
        encrypted.push(if last == 'f' { '0' } else { 'f' });
        assert!(decrypt(&cipher, &encrypted).is_err());
    }

    #[test]
    fn blob_too_short_is_rejected() {
        let cipher = test_cipher();
        // 27 hex chars = 13 bytes raw, which is less than 12 (nonce) + 16 (tag) minimum.
        assert!(decrypt(&cipher, "aabbccddeeff001122334455667").is_err());
    }

    #[test]
    fn short_secret_is_padded() {
        // A 4-byte secret should still produce a usable cipher (padded with zeros).
        let cipher = cipher_from_secret("tiny");
        let encrypted = encrypt(&cipher, "data").unwrap();
        assert_eq!(decrypt(&cipher, &encrypted).unwrap(), "data");
    }

    #[test]
    fn key_fingerprint_is_stable_and_distinguishes_secrets() {
        // Same secret → same fingerprint (the property that makes it useful for
        // confirming the key didn't change across restarts).
        assert_eq!(key_fingerprint("my-secret"), key_fingerprint("my-secret"));
        assert_ne!(key_fingerprint("key-a"), key_fingerprint("key-b"));
        // 8 hex chars (4 bytes) and never the raw secret.
        let fp = key_fingerprint("my-secret");
        assert_eq!(fp.len(), 8);
        assert!(!fp.contains("my-secret"));
    }

    #[test]
    fn key_fingerprint_exposes_the_32_byte_truncation() {
        // Two secrets identical in their first 32 bytes but differing afterwards
        // derive the SAME key — the fingerprint makes that latent footgun visible.
        let a = "0123456789abcdef0123456789abcdef".to_string() + "TAIL-A";
        let b = "0123456789abcdef0123456789abcdef".to_string() + "TAIL-B";
        assert_eq!(key_fingerprint(&a), key_fingerprint(&b));
    }
}
