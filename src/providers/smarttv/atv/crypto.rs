//! Cryptographic identity + pairing-secret math for Android TV Remote v2.
//!
//! The client authenticates with a self-signed **RSA-2048** certificate (the
//! credential persisted after pairing). Pairing proves possession of that key by
//! hashing both peers' public keys with the on-screen code — see
//! [`pairing_secret`], which reproduces the algorithm the TV firmware expects.

use anyhow::{Result, anyhow, bail};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};
use std::str::FromStr;
use x509_cert::der::{Decode, DecodePem, Encode, EncodePem};

const RSA_BITS: usize = 2048;

/// A freshly generated, or restored, client identity (self-signed RSA cert +
/// its private key), held as PEM for persistence in the provider credentials.
#[derive(Clone)]
pub struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
}

impl Identity {
    /// Generate a new self-signed RSA-2048 identity. The subject/issuer is a
    /// fixed CN — the TV pairs on the key, not the name.
    pub fn generate() -> Result<Self> {
        use rsa::pkcs1v15::SigningKey;
        use x509_cert::builder::{Builder, CertificateBuilder, Profile};
        use x509_cert::name::Name;
        use x509_cert::serial_number::SerialNumber;
        use x509_cert::spki::SubjectPublicKeyInfoOwned;
        use x509_cert::time::Validity;

        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, RSA_BITS)?;
        let public = RsaPublicKey::from(&private);

        let signer = SigningKey::<Sha256>::new(private.clone());
        let spki = SubjectPublicKeyInfoOwned::from_key(public)?;
        let serial = SerialNumber::from(1u32);
        let validity = Validity::from_now(std::time::Duration::from_secs(20 * 365 * 24 * 3600))?;
        let subject = Name::from_str("CN=bifrost-atvremote")?;
        let builder = CertificateBuilder::new(
            Profile::Root, // self-signed root (issuer = subject)
            serial,
            validity,
            subject,
            spki,
            &signer,
        )?;
        let cert = builder.build()?;

        Ok(Identity {
            cert_pem: cert.to_pem(LineEnding::LF)?,
            key_pem: private.to_pkcs8_pem(LineEnding::LF)?.to_string(),
        })
    }

    /// The certificate's DER bytes (for the pairing-secret hash and for rustls).
    pub fn cert_der(&self) -> Result<Vec<u8>> {
        Ok(x509_cert::Certificate::from_pem(self.cert_pem.as_bytes())?.to_der()?)
    }

    /// The private key's PKCS#8 DER bytes (for the rustls client config).
    pub fn key_der(&self) -> Result<Vec<u8>> {
        Ok(RsaPrivateKey::from_pkcs8_pem(&self.key_pem)?
            .to_pkcs8_der()?
            .as_bytes()
            .to_vec())
    }
}

/// Big-endian modulus and exponent of the RSA public key in a DER certificate.
fn rsa_modulus_exponent(cert_der: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    use rsa::pkcs8::DecodePublicKey;
    let cert = x509_cert::Certificate::from_der(cert_der)?;
    let spki_der = cert.tbs_certificate.subject_public_key_info.to_der()?;
    let pk = RsaPublicKey::from_public_key_der(&spki_der)
        .map_err(|e| anyhow!("certificate public key is not RSA: {e}"))?;
    Ok((pk.n().to_bytes_be(), pk.e().to_bytes_be()))
}

/// Compute the pairing secret the TV expects, given both certificates and the
/// on-screen `code` (hex, e.g. `"1A2B3C"`).
///
/// `SHA-256(client_modulus ‖ client_exponent ‖ server_modulus ‖ server_exponent
/// ‖ nonce)`, where the first byte of the code is a checksum over the rest and
/// `nonce` is the remaining code bytes. Returns an error (wrong code) if the
/// checksum doesn't match — caught before anything is sent to the TV.
pub fn pairing_secret(
    client_cert_der: &[u8],
    server_cert_der: &[u8],
    code: &str,
) -> Result<Vec<u8>> {
    let code = code.trim();
    if code.len() < 2 || !code.len().is_multiple_of(2) {
        bail!("pairing code must be an even number of hex digits");
    }
    let check = u8::from_str_radix(&code[..2], 16).map_err(|_| anyhow!("pairing code not hex"))?;
    let nonce = hex::decode(&code[2..]).map_err(|_| anyhow!("pairing code not hex"))?;

    let (cn, ce) = rsa_modulus_exponent(client_cert_der)?;
    let (sn, se) = rsa_modulus_exponent(server_cert_der)?;

    let mut h = Sha256::new();
    h.update(&cn);
    h.update(&ce);
    h.update(&sn);
    h.update(&se);
    h.update(&nonce);
    let digest = h.finalize();

    if digest[0] != check {
        bail!("pairing code did not match — re-enter the code shown on the TV");
    }
    Ok(digest.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_identity_roundtrips_to_der() {
        let id = Identity::generate().expect("generate");
        assert!(id.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(id.key_pem.contains("PRIVATE KEY"));
        // DER conversions parse cleanly.
        let cert_der = id.cert_der().expect("cert der");
        assert!(!cert_der.is_empty());
        assert!(!id.key_der().expect("key der").is_empty());
        // The cert carries a 2048-bit RSA key (256-byte modulus, exponent 65537).
        let (n, e) = rsa_modulus_exponent(&cert_der).expect("rsa parts");
        assert_eq!(n.len(), 256, "2048-bit modulus");
        assert_eq!(e, vec![0x01, 0x00, 0x01], "exponent 65537");
    }

    #[test]
    fn pairing_secret_matches_reference_algorithm() {
        let client = Identity::generate().unwrap();
        let server = Identity::generate().unwrap();
        let cder = client.cert_der().unwrap();
        let sder = server.cert_der().unwrap();
        let (cn, ce) = rsa_modulus_exponent(&cder).unwrap();
        let (sn, se) = rsa_modulus_exponent(&sder).unwrap();

        // Pick an arbitrary nonce, compute the expected digest the way the TV
        // does, then build the code as <checkbyte><nonce-hex> and confirm
        // pairing_secret reproduces the same digest.
        let nonce = [0x2b_u8, 0x3c];
        let mut h = Sha256::new();
        h.update(&cn);
        h.update(&ce);
        h.update(&sn);
        h.update(&se);
        h.update(nonce);
        let expected = h.finalize().to_vec();
        let code = format!("{:02X}{}", expected[0], hex::encode(nonce));

        let got = pairing_secret(&cder, &sder, &code).expect("secret");
        assert_eq!(got, expected);
    }

    #[test]
    fn pairing_secret_rejects_wrong_check_byte() {
        let client = Identity::generate().unwrap();
        let server = Identity::generate().unwrap();
        let cder = client.cert_der().unwrap();
        let sder = server.cert_der().unwrap();
        // "00" check byte almost certainly won't match the real digest[0].
        let err = pairing_secret(&cder, &sder, "002b3c").unwrap_err();
        assert!(err.to_string().contains("did not match"), "{err}");
    }

    #[test]
    fn pairing_secret_rejects_malformed_code() {
        let id = Identity::generate().unwrap();
        let d = id.cert_der().unwrap();
        assert!(pairing_secret(&d, &d, "abc").is_err()); // odd length
        assert!(pairing_secret(&d, &d, "zz11").is_err()); // not hex
    }
}
