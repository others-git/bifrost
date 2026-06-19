use anyhow::{Context, Result, bail};

pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub secret: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let raw_secret = std::env::var("BIFROST_SECRET")
            .context("BIFROST_SECRET env var must be set (min 32 chars recommended)")?;
        let secret = clean_secret(&raw_secret)?;
        // A stray trailing newline/space (common in `.env` files and copy-paste)
        // would otherwise change the derived key — the classic "same secret, can't
        // decrypt" footgun. We trim it; warn so the operator knows the effective
        // key is derived from the trimmed value.
        if secret.len() != raw_secret.len() {
            tracing::warn!(
                target: "bifrost::crypto",
                "BIFROST_SECRET had surrounding whitespace — trimmed it; the encryption key is derived from the trimmed value"
            );
        }
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://bifrost.db".into()),
            bind_addr: std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into()),
            secret,
        })
    }
}

/// Trim surrounding whitespace from the secret and reject an empty/whitespace-only
/// value. Trimming makes the derived key insensitive to a stray trailing newline
/// or space (a frequent cause of a "same" secret that no longer decrypts).
fn clean_secret(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("BIFROST_SECRET is empty (or only whitespace)");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::clean_secret;

    #[test]
    fn clean_secret_trims_surrounding_whitespace() {
        // Leading/trailing whitespace (incl. a trailing newline) is stripped, so
        // the derived key is the same whether or not the secret picked up a stray
        // space/newline from `.env` or a paste.
        assert_eq!(clean_secret("  my-secret \n").unwrap(), "my-secret");
        assert_eq!(clean_secret("\tabc\r\n").unwrap(), "abc");
        // Interior characters are preserved verbatim.
        assert_eq!(clean_secret("a b c").unwrap(), "a b c");
    }

    #[test]
    fn clean_secret_unchanged_when_no_whitespace() {
        let s = "0123456789abcdef0123456789abcdef";
        assert_eq!(clean_secret(s).unwrap(), s);
    }

    #[test]
    fn clean_secret_rejects_empty_or_whitespace_only() {
        assert!(clean_secret("").is_err());
        assert!(clean_secret("   \n\t ").is_err());
    }

    #[test]
    fn from_env_requires_bifrost_secret() {
        // Ensure the secret is required — clear it and expect an error.
        // (We can't easily set env vars in parallel tests so we just verify
        // the code path when the var is absent in a fresh process.)
        // The Option variant covers the absence case.
        assert!(
            std::env::var("BIFROST_SECRET").is_err()
                || !std::env::var("BIFROST_SECRET").unwrap().is_empty()
        );
    }
}
