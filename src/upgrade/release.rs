use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub(super) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fetch(url: &str, limit: u64) -> io::Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("User-Agent", "outrim-upgrade")
        .call()
        .map_err(io::Error::other)?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("release response is too large"));
    }
    Ok(bytes)
}

fn asset_name() -> io::Result<String> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc.exe",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return Err(io::Error::other("no release asset for this platform")),
    };
    Ok(format!("outrim-{target}"))
}

/// Resolve latest once, then use only asset URLs from that immutable release.
pub(super) fn prepare(work: &Path, executable: &Path) -> io::Result<Option<PathBuf>> {
    let api = std::env::var("OUTRIM_RELEASE_API")
        .unwrap_or_else(|_| "https://api.github.com/repos/WodenJay/Outrim/releases/latest".into());
    let release: Release =
        serde_json::from_slice(&fetch(&api, 1024 * 1024)?).map_err(io::Error::other)?;
    let version = semver::Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(io::Error::other)?;
    if release.prerelease || release.draft || !version.pre.is_empty() {
        return Err(io::Error::other("release is not stable"));
    }
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(io::Error::other)?;
    if version <= current {
        println!("Keeping binary {current}; latest stable is {version} (no downgrade).");
        return Ok(None);
    }
    let name = asset_name()?;
    let url = |name: &str| -> io::Result<&str> {
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| io::Error::other(format!("release has no {name}")))?;
        if !asset
            .browser_download_url
            .contains(&format!("/download/{}/", release.tag_name))
        {
            return Err(io::Error::other(
                "release asset URL is not pinned to the selected tag",
            ));
        }
        Ok(&asset.browser_download_url)
    };
    println!(
        "Preparing stable release {} for {}",
        release.tag_name,
        executable.display()
    );
    let sums =
        String::from_utf8(fetch(url("SHA256SUMS")?, 1024 * 1024)?).map_err(io::Error::other)?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            (fields.next()?.trim_start_matches('*') == name).then_some(hash)
        })
        .ok_or_else(|| io::Error::other("SHA256 checksum is missing"))?;
    let bytes = fetch(url(&name)?, 256 * 1024 * 1024)?;
    if bytes.is_empty() || !digest(&bytes).eq_ignore_ascii_case(expected) {
        return Err(io::Error::other(
            "SHA256 checksum mismatch; installation unchanged",
        ));
    }
    let path = work.join(format!("release{}", std::env::consts::EXE_SUFFIX));
    let mut file = fs::File::create(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::set_permissions(&path, fs::metadata(executable)?.permissions())?;
    Ok(Some(path))
}
