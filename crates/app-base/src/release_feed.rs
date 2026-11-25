#![cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]

use std::cmp::Ordering;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Duration;

use futures_util::AsyncReadExt as _;
use gpui_kit::http_client::{
    AsyncBody, HttpRequestExt as _, RedirectPolicy, Request, Response, Result as HttpResult, Url,
};
use gpui_kit::{App, Task};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::updater::UpdatePaths;

pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const REPOSITORY: &str = "matteolomba/rusty-teams";
pub const RELEASES_URL: &str =
    "https://api.github.com/repos/matteolomba/rusty-teams/releases?per_page=10";
pub const RELEASES_PAGE_URL: &str = "https://github.com/matteolomba/rusty-teams/releases";
pub const RELEASE_TAG_URL: &str = "https://github.com/matteolomba/rusty-teams/releases/tag/";
/// Only assets hosted by this fork may be downloaded; upstream executables are ignored.
pub const RELEASE_DOWNLOAD_URL: &str =
    "https://github.com/matteolomba/rusty-teams/releases/download/";

/// Where the release page of `tag` can be opened without downloading anything.
pub fn release_page_url(tag: &str) -> String {
    format!("{RELEASE_TAG_URL}{tag}")
}

/// The release asset this platform installs, and the magic bytes it must start with.
#[cfg(test)]
fn asset_suffix_for(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("windows", "aarch64") => "-windows-arm64.exe",
        ("windows", _) => "-windows-amd64.exe",
        ("linux", _) => "-linux-amd64",
        _ => "-unsupported",
    }
}

#[cfg(all(windows, target_arch = "aarch64"))]
const ASSET_SUFFIX: &str = "-windows-arm64.exe";
#[cfg(all(windows, not(target_arch = "aarch64")))]
const ASSET_SUFFIX: &str = "-windows-amd64.exe";
#[cfg(target_os = "linux")]
const ASSET_SUFFIX: &str = "-linux-amd64";
#[cfg(not(any(windows, target_os = "linux")))]
const ASSET_SUFFIX: &str = "-unsupported";
#[cfg(windows)]
const EXECUTABLE_MAGIC: &[u8] = b"MZ";
#[cfg(target_os = "linux")]
const EXECUTABLE_MAGIC: &[u8] = b"\x7fELF";
/// macOS and any other target still compile. The suffix matches no release asset,
/// so the feed stays idle. macOS updates by replacing the `.app`, not a Mach-O in place.
#[cfg(not(any(windows, target_os = "linux")))]
const EXECUTABLE_MAGIC: &[u8] = b"\0unsupported";
const PARTIAL_SUFFIX: &str = ".partial";
const DOWNLOAD_CHUNK: usize = 64 * 1024;
const SHA256_PREFIX: &str = "sha256:";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DOWNLOAD_SIZE: u64 = 128 * 1024 * 1024;
const MAX_METADATA_SIZE: u64 = 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const METADATA_HOSTS: [&str; 1] = ["api.github.com"];

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseVersion {
    core: [u64; 3],
    pre_release: Vec<String>,
}

fn parse_version(tag: &str) -> Option<ReleaseVersion> {
    let text = tag.trim().strip_prefix('v')?;
    let (core_text, pre_text) = match text.split_once('-') {
        Some((core_text, pre_text)) => (core_text, Some(pre_text)),
        None => (text, None),
    };
    let mut parts = core_text.split('.').map(|part| part.parse::<u64>().ok());
    let core = [parts.next()??, parts.next()??, parts.next()??];
    if parts.next().is_some() {
        return None;
    }
    let pre_release = pre_text
        .map(|pre_text| pre_text.split('.').map(str::to_owned).collect())
        .unwrap_or_default();
    Some(ReleaseVersion { core, pre_release })
}

fn compare_identifiers(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre_release.is_empty(), other.pre_release.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self
                    .pre_release
                    .iter()
                    .zip(&other.pre_release)
                    .map(|(left, right)| compare_identifiers(left, right))
                    .find(|ordering| ordering.is_ne())
                    .unwrap_or_else(|| self.pre_release.len().cmp(&other.pre_release.len())),
            }
        })
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether release `tag` is newer than `running_tag`.
pub fn is_newer(tag: &str, running_tag: &str) -> bool {
    matches!(
        (parse_version(tag), parse_version(running_tag)),
        (Some(tag), Some(running)) if tag > running
    )
}

/// A newer tag, even when this platform has no installable asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedRelease {
    pub tag: String,
}

/// The newest release newer than `running_tag`, ignoring whether an asset exists.
pub fn newest_release<'a>(running_tag: &str, releases: &'a [Release]) -> Option<&'a Release> {
    let running = parse_version(running_tag)?;
    let follows_pre_releases = !running.pre_release.is_empty();
    releases
        .iter()
        .filter(|release| !release.draft && (follows_pre_releases || !release.prerelease))
        .filter_map(|release| {
            let version = parse_version(&release.tag_name).filter(|version| *version > running)?;
            Some((version, release))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, release)| release)
}

/// Pre-releases are offered only to a build that runs a pre-release itself.
pub fn newest_update<'a>(
    running_tag: &str,
    releases: &'a [Release],
) -> Option<(&'a Release, &'a ReleaseAsset)> {
    let running = parse_version(running_tag)?;
    let follows_pre_releases = !running.pre_release.is_empty();
    releases
        .iter()
        .filter(|release| !release.draft && (follows_pre_releases || !release.prerelease))
        .filter_map(|release| {
            let version = parse_version(&release.tag_name).filter(|version| *version > running)?;
            let asset = release.assets.iter().find(|asset| {
                asset.name.ends_with(ASSET_SUFFIX)
                    && asset.browser_download_url.starts_with(RELEASE_DOWNLOAD_URL)
            })?;
            Some((version, release, asset))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, release, asset)| (release, asset))
}

fn verify_digest(expected: &str, bytes: &[u8]) -> Result<(), String> {
    let Some(hex) = expected.strip_prefix(SHA256_PREFIX) else {
        return Err("release asset has no sha256 digest".to_owned());
    };
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(hex) {
        return Err("sha256 mismatch".to_owned());
    }
    Ok(())
}

/// Re-checks the staged executable against the digest saved at download time,
/// immediately before the swap. A tampered local file must not be installed.
pub fn verify_staged(paths: &UpdatePaths) -> Result<(), String> {
    let expected = fs::read_to_string(&paths.digest_file)
        .map_err(|error| format!("digest del file staged: {error}"))?;
    let bytes = fs::read(&paths.update_exe).map_err(|error| format!("file staged: {error}"))?;
    if !bytes.starts_with(EXECUTABLE_MAGIC) {
        return Err("il file staged non è un eseguibile per questa piattaforma".to_owned());
    }
    verify_digest(expected.trim(), &bytes)
}

pub fn verify_download(asset: &ReleaseAsset, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != asset.size {
        return Err(format!("size {} instead of {}", bytes.len(), asset.size));
    }
    if !bytes.starts_with(EXECUTABLE_MAGIC) {
        return Err("not an executable for this platform".to_owned());
    }
    let Some(expected) = asset.digest.as_deref() else {
        return Err("release asset has no sha256 digest".to_owned());
    };
    verify_digest(expected, bytes)
}

fn partial_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(PARTIAL_SUFFIX);
    path.with_file_name(name)
}

fn already_staged(paths: &UpdatePaths, tag: &str) -> bool {
    paths.update_exe.exists()
        && fs::read_to_string(&paths.version_file).is_ok_and(|staged| staged.trim() == tag)
}

fn stage(paths: &UpdatePaths, tag: &str, bytes: &[u8], digest: &str) -> Result<(), String> {
    if !digest.starts_with(SHA256_PREFIX) {
        return Err("release asset has no sha256 digest".to_owned());
    }
    let directory = paths
        .update_exe
        .parent()
        .ok_or_else(|| "no update directory".to_owned())?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&paths.version_file);
    let partial = partial_path(&paths.update_exe);
    fs::write(&partial, bytes).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    fs::rename(&partial, &paths.update_exe).map_err(|error| error.to_string())?;
    let digest_partial = partial_path(&paths.digest_file);
    let _ = fs::remove_file(&paths.digest_file);
    fs::write(&digest_partial, digest).map_err(|error| error.to_string())?;
    fs::rename(&digest_partial, &paths.digest_file).map_err(|error| error.to_string())?;
    fs::write(&paths.version_file, tag).map_err(|error| error.to_string())
}

/// A newer release this platform can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub tag: String,
    pub asset: ReleaseAsset,
}

type HttpClient = std::sync::Arc<dyn gpui_kit::http_client::HttpClient>;

/// Only HTTPS on a fixed set of fork hosts may be fetched, including redirects.
fn validate_https_url(url: &str, hosts: &[&str]) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|error| format!("URL non valida: {error}"))?;
    if parsed.scheme() != "https" {
        return Err("sono ammessi solo URL HTTPS".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL con credenziali non ammesso".to_owned());
    }
    if parsed.port().is_some() {
        return Err("URL con porta esplicita non ammesso".to_owned());
    }
    if !hosts.iter().any(|host| parsed.host_str() == Some(*host)) {
        return Err("host di download non consentito".to_owned());
    }
    Ok(parsed)
}

/// Incremental bound: an oversized chunk fails before it is appended to the body.
fn checked_body_len(current: usize, read: usize, limit: u64) -> Result<usize, String> {
    let total = (current as u64)
        .checked_add(read as u64)
        .ok_or_else(|| "body too large".to_owned())?;
    if total > limit {
        return Err(format!("body exceeds {limit} bytes"));
    }
    usize::try_from(total).map_err(|_| "body too large".to_owned())
}

async fn read_body(
    body: &mut AsyncBody,
    progress: Option<&AtomicU64>,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0; DOWNLOAD_CHUNK];
    loop {
        let read = body
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(bytes);
        }
        let total = checked_body_len(bytes.len(), read, limit)?;
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(progress) = progress {
            progress.store(total as u64, AtomicOrdering::Relaxed);
        }
    }
}

/// GETs `url` with an explicit timeout and no automatic redirects: every hop is
/// validated against `hosts` and the body is bounded incrementally.
async fn get(
    client: &HttpClient,
    url: &str,
    hosts: &[&str],
    progress: Option<&AtomicU64>,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let mut next = validate_https_url(url, hosts)?.to_string();
    let limit = max_bytes.min(MAX_DOWNLOAD_SIZE);
    for _ in 0..=MAX_REDIRECTS {
        let request = Request::builder()
            .uri(next.as_str())
            .follow_redirects(RedirectPolicy::NoFollow)
            .timeout(DOWNLOAD_TIMEOUT)
            .body(().into())
            .map_err(|error| error.to_string())?;
        let mut response = client
            .send(request)
            .await
            .map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(gpui_kit::http_client::http::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "redirect senza destinazione".to_owned())?;
            next = validate_https_url(location, hosts)?.to_string();
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("status {}", response.status()));
        }
        return read_body(response.body_mut(), progress, limit).await;
    }
    Err("too many redirects".to_owned())
}

/// Looks for a newer tag, without requiring an asset and without downloading.
pub fn find_release(
    running_tag: &'static str,
    cx: &App,
) -> Task<Result<Option<TaggedRelease>, String>> {
    let client = cx.http_client();
    cx.background_executor().spawn(async move {
        let releases: Vec<Release> = serde_json::from_slice(
            &get(
                &client,
                RELEASES_URL,
                &METADATA_HOSTS,
                None,
                MAX_METADATA_SIZE,
            )
            .await?,
        )
        .map_err(|error| error.to_string())?;
        Ok(
            newest_release(running_tag, &releases).map(|release| TaggedRelease {
                tag: release.tag_name.clone(),
            }),
        )
    })
}

/// Looks for a release newer than `running_tag`, without downloading it.
pub fn find_update(
    running_tag: &'static str,
    cx: &App,
) -> Task<Result<Option<AvailableUpdate>, String>> {
    let client = cx.http_client();
    cx.background_executor().spawn(async move {
        let releases: Vec<Release> = serde_json::from_slice(
            &get(
                &client,
                RELEASES_URL,
                &METADATA_HOSTS,
                None,
                MAX_METADATA_SIZE,
            )
            .await?,
        )
        .map_err(|error| error.to_string())?;
        Ok(
            newest_update(running_tag, &releases).map(|(release, asset)| AvailableUpdate {
                tag: release.tag_name.clone(),
                asset: asset.clone(),
            }),
        )
    })
}

/// Downloads and verifies `update` into the update folder, counting bytes in `progress`.
pub fn download_update(
    update: AvailableUpdate,
    paths: UpdatePaths,
    progress: std::sync::Arc<AtomicU64>,
    cx: &App,
) -> Task<Result<(), String>> {
    let client = cx.http_client();
    cx.background_executor().spawn(async move {
        if !crate::updater::self_update_allowed() {
            return Err(
                "aggiornamento automatico non disponibile per questa installazione".to_owned(),
            );
        }
        if already_staged(&paths, &update.tag) {
            progress.store(update.asset.size, AtomicOrdering::Relaxed);
            return Ok(());
        }
        let digest = update.asset.digest.clone().unwrap_or_default();
        let bytes = get(
            &client,
            &update.asset.browser_download_url,
            &DOWNLOAD_HOSTS,
            Some(&progress),
            update.asset.size,
        )
        .await?;
        verify_download(&update.asset, &bytes)?;
        stage(&paths, &update.tag, &bytes, &digest)
    })
}

/// Downloads a newer GitHub release into the update folder; the regular update poll installs it.
pub fn fetch_update(
    running_tag: &'static str,
    paths: UpdatePaths,
    cx: &App,
) -> Task<Result<Option<String>, String>> {
    let find = find_update(running_tag, cx);
    let client = cx.http_client();
    cx.background_executor().spawn(async move {
        let Some(update) = find.await? else {
            return Ok(None);
        };
        if !crate::updater::self_update_allowed() {
            return Err(
                "aggiornamento automatico non disponibile per questa installazione".to_owned(),
            );
        }
        if already_staged(&paths, &update.tag) {
            return Ok(None);
        }
        let digest = update.asset.digest.clone().unwrap_or_default();
        let bytes = get(
            &client,
            &update.asset.browser_download_url,
            &DOWNLOAD_HOSTS,
            None,
            update.asset.size,
        )
        .await?;
        verify_download(&update.asset, &bytes)?;
        stage(&paths, &update.tag, &bytes, &digest)?;
        Ok(Some(update.tag))
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::sync::{Arc, Mutex};

    fn digest_of(bytes: &[u8]) -> String {
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{SHA256_PREFIX}{digest}")
    }

    /// Published asset name drops the leading `v` (`rusty-teams-0.5.0-windows-amd64.exe`).
    fn asset_name(tag: &str) -> String {
        let version = tag.strip_prefix('v').unwrap_or(tag);
        format!("rusty-teams-{version}{ASSET_SUFFIX}")
    }

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease,
            assets: vec![ReleaseAsset {
                name: asset_name(tag),
                browser_download_url: format!("{RELEASE_DOWNLOAD_URL}{tag}/{tag}.exe"),
                size: EXECUTABLE_MAGIC.len() as u64,
                digest: None,
            }],
        }
    }

    #[test]
    fn windows_arm_and_amd64_look_for_different_exes() {
        assert_eq!(asset_suffix_for("windows", "aarch64"), "-windows-arm64.exe");
        assert_eq!(asset_suffix_for("windows", "x86_64"), "-windows-amd64.exe");
        assert!(
            !"rusty-teams-0.5.0-windows-amd64.exe"
                .ends_with(asset_suffix_for("windows", "aarch64"))
        );
        assert!(
            "rusty-teams-0.5.0-windows-arm64.exe".ends_with(asset_suffix_for("windows", "aarch64"))
        );
    }

    #[test]
    fn the_installed_suffix_matches_this_target() {
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "other"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        assert_eq!(ASSET_SUFFIX, asset_suffix_for(os, arch));
    }

    #[test]
    fn versions_order_like_semver() {
        let version = |tag| parse_version(tag).unwrap();
        assert!(version("v0.1.0") > version("v0.1.0-rc.2"));
        assert!(version("v0.1.0-rc.10") > version("v0.1.0-rc.2"));
        assert!(version("v0.2.0") > version("v0.1.9"));
        assert!(version("v1.0.0-rc.1") > version("v0.9.0"));
        assert_eq!(parse_version("0.1.0"), None);
        assert_eq!(parse_version("v0.1"), None);
    }

    #[test]
    fn a_stable_build_takes_the_newest_stable_release_only() {
        let releases = [
            release("v0.3.0-rc.1", true),
            release("v0.2.0", false),
            release("v0.1.0", false),
        ];
        let (found, asset) = newest_update("v0.1.0", &releases).unwrap();
        assert_eq!(found.tag_name, "v0.2.0");
        assert!(asset.name.ends_with(ASSET_SUFFIX));
        assert!(newest_update("v0.2.0", &releases).is_none());
        let (found, _) = newest_update("v0.2.0-rc.1", &releases).unwrap();
        assert_eq!(found.tag_name, "v0.3.0-rc.1");
    }

    #[test]
    fn a_release_without_a_windows_exe_or_a_dev_build_gets_nothing() {
        let mut bare = release("v0.2.0", false);
        bare.assets.clear();
        assert!(newest_update("v0.1.0", &[bare.clone()]).is_none());
        assert_eq!(
            newest_release("v0.1.0", &[bare]).map(|release| release.tag_name.as_str()),
            Some("v0.2.0")
        );
        assert!(newest_update("0356d1d-20261107T101752Z", &[release("v0.2.0", false)]).is_none());
    }

    #[test]
    fn the_release_page_is_the_tag_url() {
        assert_eq!(
            release_page_url("v0.5.0"),
            "https://github.com/danieldaeschle/rusty-teams/releases/tag/v0.5.0"
        );
    }

    #[test]
    fn a_download_must_match_size_header_and_digest() {
        let bytes = EXECUTABLE_MAGIC;
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut asset = release("v0.2.0", false).assets.remove(0);
        assert!(verify_download(&asset, bytes).is_err());
        asset.digest = Some(format!("{SHA256_PREFIX}{digest}"));
        assert!(verify_download(&asset, bytes).is_ok());
        assert!(verify_download(&asset, b"XY").is_err());
        assert!(verify_download(&asset, &[EXECUTABLE_MAGIC, b"!"].concat()).is_err());
    }

    #[test]
    fn newer_compares_release_versions() {
        assert!(is_newer("v0.5.0", "v0.4.0"));
        assert!(is_newer("v0.5.0", "v0.5.0-rc.1"));
        assert!(!is_newer("v0.4.0", "v0.4.0"));
        assert!(!is_newer("v0.3.9", "v0.4.0"));
        assert!(!is_newer("garbage", "v0.4.0"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_takes_the_plain_binary_not_the_tarball() {
        let mut release = release("v0.5.0", false);
        let binary = release.assets.remove(0);
        let tarball = ReleaseAsset {
            name: "rusty-teams-0.5.0-linux-amd64.tar.gz".to_owned(),
            ..binary.clone()
        };
        release.assets = vec![tarball.clone()];
        assert!(newest_update("v0.4.0", std::slice::from_ref(&release)).is_none());
        release.assets = vec![tarball, binary];
        let (_, asset) = newest_update("v0.4.0", std::slice::from_ref(&release)).unwrap();
        assert_eq!(asset.name, "rusty-teams-0.5.0-linux-amd64");
    }

    #[test]
    fn staging_writes_the_exe_before_the_version() {
        let directory = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::beside(&directory.path().join("teams"));
        assert!(!already_staged(&paths, "v0.2.0"));
        stage(
            &paths,
            "v0.2.0",
            EXECUTABLE_MAGIC,
            &digest_of(EXECUTABLE_MAGIC),
        )
        .unwrap();
        assert_eq!(fs::read(&paths.update_exe).unwrap(), EXECUTABLE_MAGIC);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&paths.update_exe)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        assert!(already_staged(&paths, "v0.2.0"));
        assert_eq!(
            fs::read_to_string(&paths.digest_file).unwrap(),
            digest_of(EXECUTABLE_MAGIC)
        );
        assert!(!already_staged(&paths, "v0.3.0"));
        assert!(!partial_path(&paths.update_exe).exists());
    }

    #[test]
    fn the_feed_and_release_page_point_at_the_fork() {
        assert!(RELEASES_URL.contains("repos/matteolomba/rusty-teams/"));
        assert!(!RELEASES_URL.contains("danieldaeschle"));
        assert_eq!(
            RELEASES_PAGE_URL,
            "https://github.com/matteolomba/rusty-teams/releases"
        );
        assert_eq!(
            release_page_url("v0.7.0-rc.1"),
            "https://github.com/matteolomba/rusty-teams/releases/tag/v0.7.0-rc.1"
        );
    }

    #[test]
    fn only_assets_hosted_by_the_fork_are_selected() {
        let fork = release("v0.7.0-rc.1", true);
        let mut upstream = release("v0.7.1-rc.1", true);
        upstream.assets[0].browser_download_url = upstream.assets[0].browser_download_url.replace(
            RELEASE_DOWNLOAD_URL,
            "https://github.com/danieldaeschle/rusty-teams/releases/download/",
        );
        let releases = [upstream.clone(), fork];
        let (found, asset) = newest_update("v0.7.0-rc.0", &releases).unwrap();
        assert_eq!(found.tag_name, "v0.7.0-rc.1");
        assert_eq!(
            asset.browser_download_url,
            format!("{RELEASE_DOWNLOAD_URL}v0.7.0-rc.1/v0.7.0-rc.1.exe")
        );
        assert!(newest_update("v0.7.0-rc.1", std::slice::from_ref(&upstream)).is_none());
    }

    #[test]
    fn staging_keeps_the_release_tag() {
        let directory = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::beside(&directory.path().join("teams"));
        stage(
            &paths,
            "v0.7.0-rc.1",
            EXECUTABLE_MAGIC,
            &digest_of(EXECUTABLE_MAGIC),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&paths.version_file).unwrap(),
            "v0.7.0-rc.1"
        );
        assert!(already_staged(&paths, "v0.7.0-rc.1"));
        assert!(!already_staged(&paths, "v0.7.1-rc.1"));
    }

    #[test]
    fn staged_release_is_reverified_against_the_saved_digest() {
        let directory = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::beside(&directory.path().join("teams"));
        stage(
            &paths,
            "v0.2.0",
            EXECUTABLE_MAGIC,
            &digest_of(EXECUTABLE_MAGIC),
        )
        .unwrap();
        assert!(verify_staged(&paths).is_ok());
        fs::write(&paths.update_exe, [EXECUTABLE_MAGIC, b"!"].concat()).unwrap();
        assert!(verify_staged(&paths).is_err());
        fs::remove_file(&paths.digest_file).unwrap();
        assert!(verify_staged(&paths).is_err());
    }

    #[test]
    fn downloads_only_accept_https_on_fixed_hosts() {
        assert!(validate_https_url("https://github.com/a/b", &DOWNLOAD_HOSTS).is_ok());
        assert!(
            validate_https_url("https://objects.githubusercontent.com/a", &DOWNLOAD_HOSTS).is_ok()
        );
        assert!(validate_https_url("http://github.com/a", &DOWNLOAD_HOSTS).is_err());
        assert!(validate_https_url("https://evil.example/a", &DOWNLOAD_HOSTS).is_err());
        assert!(validate_https_url("https://github.com.evil.test/a", &DOWNLOAD_HOSTS).is_err());
        assert!(validate_https_url("https://user@github.com/a", &DOWNLOAD_HOSTS).is_err());
        assert!(validate_https_url("https://github.com:8443/a", &DOWNLOAD_HOSTS).is_err());
        assert!(validate_https_url(RELEASES_URL, &METADATA_HOSTS).is_ok());
        assert!(validate_https_url(RELEASES_URL, &DOWNLOAD_HOSTS).is_err());
    }

    #[test]
    fn body_limits_are_checked_before_appending() {
        assert_eq!(checked_body_len(4, 2, 6).unwrap(), 6);
        assert!(checked_body_len(4, 3, 6).is_err());
        assert!(checked_body_len(0, DOWNLOAD_CHUNK, MAX_DOWNLOAD_SIZE).is_ok());
        assert!(checked_body_len(MAX_DOWNLOAD_SIZE as usize, 1, MAX_DOWNLOAD_SIZE).is_err());
    }

    struct StubResponse {
        status: u16,
        location: Option<String>,
        body: Vec<u8>,
    }

    impl StubResponse {
        fn redirect(location: &str) -> Self {
            Self {
                status: 302,
                location: Some(location.to_owned()),
                body: Vec::new(),
            }
        }

        fn ok(body: &[u8]) -> Self {
            Self {
                status: 200,
                location: None,
                body: body.to_vec(),
            }
        }
    }

    struct StubClient {
        responses: Mutex<Vec<StubResponse>>,
    }

    impl StubClient {
        fn new(responses: Vec<StubResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl gpui_kit::http_client::HttpClient for StubClient {
        fn user_agent(&self) -> Option<&gpui_kit::http_client::http::HeaderValue> {
            None
        }

        fn proxy(&self) -> Option<&Url> {
            None
        }

        fn send(
            &self,
            _request: Request<AsyncBody>,
        ) -> futures_util::future::BoxFuture<'static, HttpResult<Response<AsyncBody>>> {
            let response = self
                .responses
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            let mut builder = Response::builder().status(response.status);
            if let Some(location) = response.location {
                builder = builder.header("location", location);
            }
            let built = builder.body(AsyncBody::from(response.body)).expect("stub");
            Box::pin(async move { Ok(built) })
        }
    }

    #[tokio::test]
    async fn redirects_are_followed_only_to_known_hosts() {
        let client: HttpClient = Arc::new(StubClient::new(vec![
            StubResponse::redirect("https://objects.githubusercontent.com/asset"),
            StubResponse::ok(EXECUTABLE_MAGIC),
        ]));
        let bytes = get(
            &client,
            "https://github.com/a/b",
            &DOWNLOAD_HOSTS,
            None,
            MAX_DOWNLOAD_SIZE,
        )
        .await
        .unwrap();
        assert_eq!(bytes.as_slice(), EXECUTABLE_MAGIC);

        let client: HttpClient = Arc::new(StubClient::new(vec![StubResponse::redirect(
            "https://evil.example/asset",
        )]));
        let error = get(
            &client,
            "https://github.com/a/b",
            &DOWNLOAD_HOSTS,
            None,
            MAX_DOWNLOAD_SIZE,
        )
        .await
        .unwrap_err();
        assert!(error.contains("host"), "{error}");
    }

    #[tokio::test]
    async fn too_many_redirects_and_oversized_bodies_are_rejected() {
        let redirects = (0..=MAX_REDIRECTS)
            .map(|_| StubResponse::redirect("https://github.com/next"))
            .collect();
        let client: HttpClient = Arc::new(StubClient::new(redirects));
        let error = get(
            &client,
            "https://github.com/a/b",
            &DOWNLOAD_HOSTS,
            None,
            MAX_DOWNLOAD_SIZE,
        )
        .await
        .unwrap_err();
        assert!(error.contains("redirect"), "{error}");

        let client: HttpClient = Arc::new(StubClient::new(vec![StubResponse::ok(b"ABCD")]));
        let error = get(&client, "https://github.com/a/b", &DOWNLOAD_HOSTS, None, 2)
            .await
            .unwrap_err();
        assert!(error.contains("exceeds"), "{error}");
    }
}
