//! Update check: the only part of SnapKit that uses the internet, and only when the user asks
//! (the **Check for updates** button) or turns on the automatic check, which is off by default.
//!
//! It reads the latest release from GitHub's public API. Nothing about the user or their
//! captures is sent: the request carries only SnapKit's name and version (`User-Agent`). An
//! update is installed only after a click: the installer is downloaded, checked against the
//! SHA-256 digest GitHub publishes for it, and run silently; it reopens SnapKit when it ends.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The repository the releases come from.
const REPO: &str = "MichelGSS/snapkit";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A published version.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Without the leading "v": "0.5.4".
    pub version: String,
    /// The release page, with the notes.
    pub page: String,
    /// The installer, if the release has one.
    pub installer: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex, as GitHub reports it ("digest": "sha256:…").
    pub sha256: Option<String>,
}

fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(format!("SnapKit/{CURRENT}"))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// Asks GitHub for the latest release.
pub fn latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = agent(Duration::from_secs(30))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| tf!("não foi possível consultar o GitHub: {e}", "could not reach GitHub: {e}"))?;
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse_release(&text)
}

fn parse_release(json: &str) -> Result<Release, String> {
    let bad = || t!("resposta inesperada do GitHub", "unexpected answer from GitHub").to_owned();
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| bad())?;
    let version = v["tag_name"].as_str().ok_or_else(bad)?.trim_start_matches('v').to_owned();
    let page = v["html_url"].as_str().unwrap_or_default().to_owned();
    let installer = v["assets"].as_array().and_then(|assets| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            (name.starts_with("SnapKit-") && name.ends_with("-setup.exe")).then(|| Asset {
                name: name.to_owned(),
                url: a["browser_download_url"].as_str().unwrap_or_default().to_owned(),
                size: a["size"].as_u64().unwrap_or(0),
                sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
            })
        })
    });
    Ok(Release { version, page, installer })
}

/// Is `candidate` a later version than `current`? Compares the numbers ("0.10.0" is later than
/// "0.9.9"); anything after a "-" (such as "-beta") is ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('-').next().unwrap_or_default().split('.').map(|n| n.trim().parse().unwrap_or(0)).collect()
    };
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// Was SnapKit installed with the installer (as opposed to extracted from the zip)? Only then
/// can it update itself; the portable copy opens the release page instead.
pub fn is_installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// Downloads the installer to a temporary folder and checks it against its SHA-256 digest.
/// Without a published digest it refuses: an unchecked file is never run.
pub fn download(asset: &Asset) -> Result<PathBuf, String> {
    let expected = asset
        .sha256
        .clone()
        .ok_or_else(|| t!("a versão não informa o SHA-256 do instalador", "the release doesn't give the installer's SHA-256").to_owned())?;
    if asset.name.contains(['/', '\\', ':']) {
        return Err(t!("nome de arquivo inválido", "invalid file name").to_owned());
    }
    let dir = std::env::temp_dir().join("SnapKit-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&asset.name);
    let response = agent(Duration::from_secs(15 * 60))
        .get(&asset.url)
        .call()
        .map_err(|e| tf!("não foi possível baixar a atualização: {e}", "could not download the update: {e}"))?;
    let (_, body) = response.into_parts();
    let result = save_checked(body.into_reader(), &path, &expected, asset.size);
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    result.map(|()| path)
}

/// Writes `reader` to `path` while hashing it, and compares size and digest at the end.
fn save_checked(mut reader: impl Read, path: &Path, expected: &str, size: u64) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| tf!("o download foi interrompido: {e}", "the download was cut short: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        written += n as u64;
    }
    file.flush().map_err(|e| e.to_string())?;
    if size != 0 && written != size {
        return Err(t!("o download veio incompleto", "the download is incomplete").to_owned());
    }
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != expected {
        return Err(t!(
            "o instalador baixado não confere com o SHA-256 publicado; nada foi instalado",
            "the downloaded installer doesn't match the published SHA-256; nothing was installed"
        )
        .to_owned());
    }
    Ok(())
}

/// Runs the downloaded installer silently. It closes SnapKit if it's still open, replaces the
/// files keeping the preferences and the library, and opens SnapKit again (`/relaunch=1`).
pub fn run_installer(path: &Path) -> Result<(), String> {
    std::process::Command::new(path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/relaunch=1"])
        .spawn()
        .map(drop)
        .map_err(|e| tf!("não foi possível iniciar o instalador: {e}", "could not start the installer: {e}"))
}

/// Opens a web page in the default browser.
pub fn open_page(url: &str) {
    if url.starts_with("https://") {
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.5.4", "0.5.3"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.5.3", "0.5.3"));
        assert!(!is_newer("0.5.2", "0.5.3"));
        assert!(!is_newer("0.5.3-beta", "0.5.3"));
        assert!(!is_newer("garbage", "0.5.3"));
    }

    #[test]
    fn reads_a_github_release() {
        let json = r#"{
            "tag_name": "v0.5.4",
            "html_url": "https://github.com/MichelGSS/snapkit/releases/tag/v0.5.4",
            "assets": [
                {"name": "SnapKit-0.5.4-windows-x64.zip", "browser_download_url": "https://x/zip", "size": 10, "digest": "sha256:aa"},
                {"name": "SnapKit-0.5.4-setup.exe", "browser_download_url": "https://x/setup", "size": 20, "digest": "sha256:ABCD"}
            ]
        }"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, "0.5.4");
        let setup = release.installer.unwrap();
        assert_eq!((setup.url.as_str(), setup.size), ("https://x/setup", 20));
        assert_eq!(setup.sha256.as_deref(), Some("abcd"));
        assert!(parse_release("{}").is_err());
    }

    #[test]
    fn a_download_must_match_its_digest() {
        let dir = std::env::temp_dir().join(format!("snapkit-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("setup.exe");
        // SHA-256 of "hello".
        let good = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(save_checked(&b"hello"[..], &file, good, 5).is_ok());
        assert!(save_checked(&b"hellO"[..], &file, good, 5).is_err(), "a different file is refused");
        assert!(save_checked(&b"hell"[..], &file, good, 5).is_err(), "a short download is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Asks the real GitHub. Needs the internet, so it only runs when asked for.
    #[test]
    #[ignore]
    fn reads_the_real_latest_release() {
        let release = latest().unwrap();
        println!("{release:?}");
        assert!(!release.version.is_empty());
        assert!(release.installer.is_some_and(|a| a.sha256.is_some()));
    }
}
