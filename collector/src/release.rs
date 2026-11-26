//! Release signing: an Ed25519 key in the login keychain signs each
//! release manifest (`latest.json` → `latest.json.sig`); the app carries the
//! public key and refuses an update whose manifest doesn't verify.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};

/// The keychain item (service) holding the private key, base64.
pub const KEYCHAIN_SERVICE: &str = "pathfinder-release-signing-key";

fn keychain_key() -> Result<SigningKey> {
    let raw = match std::env::var("PATHFINDER_RELEASE_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            let out = std::process::Command::new("security")
                .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
                .output()
                .context("running `security`")?;
            if !out.status.success() {
                bail!("no `{KEYCHAIN_SERVICE}` in the keychain; run `collector keygen` once");
            }
            String::from_utf8(out.stdout)?
        }
    };
    let bytes: [u8; 32] = B64
        .decode(raw.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the signing key isn't 32 bytes"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Makes a key, stores it in the login keychain and prints the public key
/// for `app/src/update.rs`. Refuses to replace an existing key. The key
/// reaches `security` on its standard input, never on a command line.
pub fn keygen() -> Result<()> {
    use std::io::Write;
    if keychain_key().is_ok() {
        bail!("a signing key already exists in the keychain; delete it first to rotate");
    }
    let mut secret = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut secret);
    let key = SigningKey::from_bytes(&secret);
    let mut child = std::process::Command::new("security")
        .arg("-i")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .context("running `security`")?;
    writeln!(
        child.stdin.take().context("no stdin for `security`")?,
        "add-generic-password -s {KEYCHAIN_SERVICE} -a pathfinder -l \"Pathfinder release signing key\" -w {}",
        B64.encode(secret)
    )?;
    if !child.wait()?.success() {
        bail!("`security add-generic-password` failed");
    }
    if keychain_key().is_err() {
        bail!("the key didn't reach the keychain");
    }
    println!("{}", B64.encode(key.verifying_key().to_bytes()));
    Ok(())
}

/// Signs `file`, writing `file.sig` (base64).
pub fn sign(file: &std::path::Path) -> Result<()> {
    let key = keychain_key()?;
    let data = std::fs::read(file)?;
    let sig = key.sign(&data);
    let out = file.with_extension(format!(
        "{}.sig",
        file.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&out, B64.encode(sig.to_bytes()))?;
    println!("signed: {}", out.display());
    Ok(())
}

/// Checks a base64 signature over `data` with a base64 public key.
#[cfg(test)]
pub fn verify(public: &str, data: &[u8], sig: &str) -> Result<()> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let public: [u8; 32] = B64
        .decode(public.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("bad public key"))?;
    let sig: [u8; 64] = B64
        .decode(sig.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("bad signature"))?;
    VerifyingKey::from_bytes(&public)?.verify(data, &Signature::from_bytes(&sig))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_and_tampering_fails() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = B64.encode(key.verifying_key().to_bytes());
        let sig = B64.encode(key.sign(b"manifest").to_bytes());
        verify(&public, b"manifest", &sig).unwrap();
        assert!(verify(&public, b"manifest!", &sig).is_err());
    }
}
