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

/// Derive an `Aes256Gcm` cipher from a raw secret string.
/// Secrets shorter than 32 bytes are zero-padded; longer ones are truncated.
pub fn cipher_from_secret(secret: &str) -> Aes256Gcm {
    let mut key_bytes = [0u8; 32];
    let src = secret.as_bytes();
    let len = src.len().min(32);
    key_bytes[..len].copy_from_slice(&src[..len]);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes))
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
}
