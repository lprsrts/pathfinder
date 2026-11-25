//! `harness update`: the newest release from the collector, verified
//! before it replaces this binary. The manifest (`latest.json`) is signed
//! with the release key, whose public half is below; each file is checked
//! against the manifest's SHA-256. Nothing updates by itself: the TUI only
//! says when a new version is out.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use pathfinder_schema::Release;
use sha2::{Digest, Sha256};

/// The release signing key's public half (`collector keygen`).
const RELEASE_PUBLIC_KEY: &str = "ezpyuTi2AfXygNX9vT4sUcST9wiKLgYoS19D2PWRMko=";

/// How often the TUI looks for a new version.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The release file for this machine.
fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => Some("universal-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

fn releases_url() -> String {
    format!("{}/releases", crate::telemetry::collector_url())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("pathfinder/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Whether version `a` is newer than `b` (dotted numbers; anything after
/// a `-` is ignored).
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(a) > parse(b)
}

fn verify(manifest: &[u8], sig: &str, public: &str) -> Result<()> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let public: [u8; 32] = B64
        .decode(public)?
        .try_into()
        .map_err(|_| anyhow!("bad public key"))?;
    let sig: [u8; 64] = B64
        .decode(sig.trim())?
        .try_into()
        .map_err(|_| anyhow!("bad signature"))?;
    VerifyingKey::from_bytes(&public)?
        .verify(manifest, &Signature::from_bytes(&sig))
        .map_err(|_| anyhow!("the release manifest's signature doesn't match; not updating"))
}

/// The newest release, after checking its signature.
pub async fn latest() -> Result<Release> {
    let http = client();
    let get = |name: &str| {
        let http = http.clone();
        let url = format!("{}/{name}", releases_url());
        async move {
            let res = http
                .get(&url)
                .send()
                .await
                .context("reaching the update server")?;
            if !res.status().is_success() {
                bail!("{url}: {}", res.status());
            }
            Ok(res.bytes().await?.to_vec())
        }
    };
    let manifest = get("latest.json").await?;
    let sig = get("latest.json.sig").await?;
    verify(
        &manifest,
        &String::from_utf8_lossy(&sig),
        RELEASE_PUBLIC_KEY,
    )?;
    Ok(serde_json::from_slice(&manifest)?)
}

/// The newer release, if there is one (for the TUI's daily check).
pub async fn available() -> Option<String> {
    if std::env::var("HARNESS_NO_UPDATE_CHECK").is_ok() {
        return None;
    }
    let stamp = stamp_path();
    let fresh = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < CHECK_EVERY);
    let version = if fresh {
        std::fs::read_to_string(&stamp).ok()?
    } else {
        let release = latest().await.ok()?;
        if let Some(dir) = stamp.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&stamp, &release.version).ok();
        release.version
    };
    newer(version.trim(), env!("CARGO_PKG_VERSION")).then(|| version.trim().to_string())
}

fn stamp_path() -> PathBuf {
    crate::config::Config::load()
        .db_path
        .with_file_name("latest-version")
}

/// `harness update [--check]`.
pub async fn run(args: &[String]) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let release = latest().await?;
    if !newer(&release.version, current) {
        println!("Pathfinder {current} is the newest version.");
        return Ok(());
    }
    println!(
        "Pathfinder {} is out (you have {current}).",
        release.version
    );
    if !release.notes.is_empty() {
        println!("\n{}\n", release.notes.trim());
    }
    if args.iter().any(|a| a == "--check") {
        println!("Run `harness update` to install it.");
        return Ok(());
    }
    let target = target().context("there's no release for this platform yet")?;
    let file = release
        .files
        .get(target)
        .with_context(|| format!("release {} has no build for {target}", release.version))?;
    let exe = std::env::current_exe()?.canonicalize()?;
    let dir = exe.parent().context("the binary has no folder")?;

    print!("Downloading {} MB… ", file.size / 1_000_000 + 1);
    let url = format!("{}/{}", releases_url(), file.path);
    let res = client().get(&url).send().await?;
    if !res.status().is_success() {
        bail!("{url}: {}", res.status());
    }
    let bytes = res.bytes().await?;
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != file.sha256 {
        bail!("the download doesn't match the signed manifest; not updating");
    }
    println!("verified.");

    let staging = dir.join(format!(".harness-update-{}", std::process::id()));
    std::fs::create_dir_all(&staging)?;
    let result = install(&bytes, &staging, &exe);
    std::fs::remove_dir_all(&staging).ok();
    result?;
    println!("Updated to {}.", release.version);
    Ok(())
}

/// Unpacks the archive and swaps the binary in place.
fn install(archive: &[u8], staging: &Path, exe: &Path) -> Result<()> {
    let tarball = staging.join("release.tar.gz");
    std::fs::write(&tarball, archive)?;
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(staging)
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("couldn't unpack the release");
    }
    let new = staging.join("harness");
    if !new.is_file() {
        bail!("the release has no `harness` binary");
    }
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&new)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            bail!("the new binary's code signature doesn't verify; not updating");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    // A rename on the same disk is atomic; the running process keeps its
    // old inode.
    std::fs::rename(&new, exe).with_context(|| format!("replacing {}", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0-rc1", "0.1.0"));
    }

    #[test]
    fn manifests_must_be_signed_by_the_release_key() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let public = B64.encode(key.verifying_key().to_bytes());
        let manifest = br#"{"version":"9.9.9","files":{}}"#;
        let sig = B64.encode(key.sign(manifest).to_bytes());
        verify(manifest, &sig, &public).unwrap();
        assert!(verify(br#"{"version":"9.9.8","files":{}}"#, &sig, &public).is_err());
        // Not signed by the real release key.
        assert!(verify(manifest, &sig, RELEASE_PUBLIC_KEY).is_err());
    }
}
