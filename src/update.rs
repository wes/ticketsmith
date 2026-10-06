//! Finding, checking and installing a newer Ticketsmith.
//!
//! Releases are signed, notarized disk images on GitHub (see `RELEASING.md`).
//! The only thing this module will install is a bundle Apple has notarized and
//! whose leaf certificate carries the same team as this copy; anything else is
//! a download that failed, however well-formed the release looked.
//!
//! The steps run in the order of the functions below: [`check`] asks what the
//! latest release is, [`download`] fetches it, [`verify`] proves it is ours,
//! and [`install`] swaps it in and relaunches. Nothing touches the installed
//! app until [`verify`] has returned.
//!
//! The network is `/usr/bin/curl` and the rest is Apple's own tools, so the
//! updater adds no HTTP or TLS stack to the app. It is macOS only, as the
//! releases are.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const RELEASES_API: &str = "https://api.github.com/repos/wes/ticketsmith/releases/latest";

/// The team releases are signed by, and the only team whose builds will be
/// installed over this one.
///
/// Hard-coded rather than read from the running bundle: reading it would let
/// anyone who replaced the app once nominate their own team for every update
/// after.
const TEAM_ID: &str = "288BJX6YHP";

/// How often a running copy looks again, after the check at launch. A ticket
/// office leaves the app open all day.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A ceiling on the disk image; the real one is under 20 MB.
const MAX_DMG_BYTES: u64 = 250 * 1024 * 1024;

/// A ceiling on the release metadata, which is a few kilobytes of JSON.
const MAX_JSON_BYTES: u64 = 1024 * 1024;

const CURL: &str = "/usr/bin/curl";

/// Whether this platform gets updates at all. Releases are macOS disk images.
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// A three-part version, which is all the tags this project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Reads `0.2.0` or `v0.2.0`, ignoring anything after the patch number so
    /// a `-beta` suffix parses as the release it is a candidate for.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let raw = raw.strip_prefix(['v', 'V']).unwrap_or(raw);
        let raw = raw.split(['-', '+']).next()?;

        let mut parts = raw.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version this build is, from `Cargo.toml`.
///
/// `TICKETSMITH_PRETEND_VERSION` overrides it, so the whole update can be tried
/// against the real latest release by an installed copy that is already on it.
pub fn current() -> Version {
    std::env::var("TICKETSMITH_PRETEND_VERSION")
        .ok()
        .and_then(|v| Version::parse(&v))
        .or_else(|| Version::parse(env!("CARGO_PKG_VERSION")))
        .unwrap_or(Version(0, 0, 0))
}

/// A release newer than this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub dmg_url: String,
    /// Where to send someone whose copy cannot update itself.
    pub page_url: String,
}

/// Asks what the latest release is, and returns it only if it is newer.
///
/// Blocking; run it off the UI thread. `Ok(None)` is the ordinary answer.
pub fn check() -> Result<Option<Release>> {
    let output = Command::new(CURL)
        .args(["--silent", "--show-error", "--location", "--max-time", "20"])
        .args(["--max-filesize", &MAX_JSON_BYTES.to_string()])
        .args(["--header", "Accept: application/vnd.github+json"])
        .args(["--user-agent", &user_agent()])
        .args(["--write-out", "\n%{http_code}"])
        .arg(RELEASES_API)
        .output()
        .context("running curl")?;
    if !output.status.success() {
        bail!("could not reach GitHub: {}", trimmed(&output.stderr));
    }

    let (body, status) =
        split_status(&output.stdout).context("curl did not report an HTTP status")?;
    // GitHub answers `releases/latest` with a 404 when there are no releases
    // yet, which from here means the same as being current.
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        bail!("GitHub returned HTTP {status}");
    }

    let json: serde_json::Value = serde_json::from_slice(body).context("reading the release")?;
    latest_from(&json)
}

/// The body `--write-out "\n%{http_code}"` leaves in front of the status.
fn split_status(output: &[u8]) -> Option<(&[u8], u16)> {
    let newline = output.iter().rposition(|b| *b == b'\n')?;
    let status = std::str::from_utf8(&output[newline + 1..]).ok()?.trim().parse().ok()?;
    Some((&output[..newline], status))
}

/// The half of [`check`] that does not need the network, so it can be tested.
fn latest_from(json: &serde_json::Value) -> Result<Option<Release>> {
    // `releases/latest` skips drafts and prereleases already; one arriving
    // anyway means something unusual happened, and doing nothing is safer.
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }

    let tag = json["tag_name"].as_str().context("the release has no tag")?;
    let version =
        Version::parse(tag).with_context(|| format!("unreadable release tag: {tag:?}"))?;
    if version <= current() {
        return Ok(None);
    }

    let dmg_url = json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asset| asset["browser_download_url"].as_str())
        .find(|url| url.to_ascii_lowercase().ends_with(".dmg"))
        .with_context(|| format!("release {tag} has no disk image attached"))?
        .to_string();

    Ok(Some(Release {
        version,
        dmg_url,
        page_url: json["html_url"].as_str().unwrap_or_default().to_string(),
    }))
}

/// Downloads the disk image into the temporary directory.
pub fn download(release: &Release) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "ticketsmith-update-{}-{}.dmg",
        release.version,
        std::process::id()
    ));
    let output = Command::new(CURL)
        .args(["--silent", "--show-error", "--fail", "--location", "--max-time", "900"])
        .args(["--max-filesize", &MAX_DMG_BYTES.to_string()])
        .args(["--user-agent", &user_agent()])
        .arg("--output")
        .arg(&path)
        .arg(&release.dmg_url)
        .output()
        .context("running curl")?;
    if !output.status.success() {
        std::fs::remove_file(&path).ok();
        bail!(
            "could not download Ticketsmith {}: {}",
            release.version,
            trimmed(&output.stderr)
        );
    }
    Ok(path)
}

/// A downloaded disk image, mounted. Dropping it unmounts the volume and
/// deletes the image, which matters most on the failure paths in [`verify`].
pub struct Mounted {
    mountpoint: PathBuf,
    dmg: PathBuf,
}

impl Mounted {
    fn app(&self) -> PathBuf {
        self.mountpoint.join("Ticketsmith.app")
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        Command::new("/usr/bin/hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(&self.mountpoint)
            .status()
            .ok();
        std::fs::remove_dir(&self.mountpoint).ok();
        std::fs::remove_file(&self.dmg).ok();
    }
}

/// Mounts the disk image and proves that what is on it is ours.
///
/// Three questions, all of which have to be answered yes:
///
/// 1. Is the signature intact? `codesign --verify --deep --strict`.
/// 2. Was it signed by *this* team? A valid Developer ID only says somebody
///    with one made it; the requirement pins the leaf certificate's team.
/// 3. Has Apple notarized it? `spctl` asks what Gatekeeper would ask.
pub fn verify(dmg: PathBuf) -> Result<Mounted> {
    let mounted = Mounted {
        mountpoint: std::env::temp_dir()
            .join(format!("ticketsmith-update-mount-{}", std::process::id())),
        dmg,
    };
    // A stale mountpoint from an earlier attempt would make hdiutil refuse.
    std::fs::remove_dir_all(&mounted.mountpoint).ok();

    let attached = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noverify", "-noautoopen", "-mountpoint"])
        .arg(&mounted.mountpoint)
        .arg(&mounted.dmg)
        .output()
        .context("running hdiutil")?;
    if !attached.status.success() {
        bail!("the disk image would not mount: {}", trimmed(&attached.stderr));
    }

    let app = mounted.app();
    if !app.join("Contents/Info.plist").is_file() {
        bail!("the disk image does not contain Ticketsmith.app");
    }
    run(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict"],
        &app,
        "the download is not intact",
    )?;
    run(
        "/usr/bin/codesign",
        &["--verify", "-R", &requirement()],
        &app,
        "the download was signed by someone else",
    )?;
    run(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute"],
        &app,
        "the download has not been notarized",
    )?;

    Ok(mounted)
}

/// The code requirement that pins a bundle to this team. The team is quoted:
/// unquoted, the requirement language reads it as a number followed by junk,
/// and the syntax error looks exactly like a foreign signature.
fn requirement() -> String {
    format!("=anchor apple generic and certificate leaf[subject.OU] = \"{TEAM_ID}\"")
}

/// Replaces the running app with the verified one and starts the new copy.
///
/// The caller should quit straight afterwards. Replacing a running app is
/// allowed (the old executable stays alive through its open file), but the
/// app on disk is no longer the one in memory.
pub fn install(mounted: &Mounted) -> Result<()> {
    let installed = installed_bundle()?;
    let parent = installed.parent().context("the app is installed at the root")?;

    // Staged beside the app, because the swap needs both on one volume.
    let staged = parent.join(format!(".ticketsmith-update-{}.app", std::process::id()));
    std::fs::remove_dir_all(&staged).ok();

    // ditto, because a bundle's signature covers its symlinks and extended
    // attributes, and this is Apple's tool for copying one intact.
    let copied = Command::new("/usr/bin/ditto")
        .arg(mounted.app())
        .arg(&staged)
        .output()
        .context("running ditto")?;
    if !copied.status.success() {
        std::fs::remove_dir_all(&staged).ok();
        bail!("could not write next to {}: {}", installed.display(), trimmed(&copied.stderr));
    }

    // Already verified more strictly than the quarantine dialog would, so the
    // relaunch should not stop to ask.
    Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged)
        .status()
        .ok();

    swap(&staged, &installed).inspect_err(|_| {
        std::fs::remove_dir_all(&staged).ok();
    })?;
    // The swap traded places, so `staged` is now the old version.
    std::fs::remove_dir_all(&staged).ok();

    // `-n` because this process has not quit yet; without it `open` would
    // bring the old copy forward instead of starting the new one.
    Command::new("/usr/bin/open")
        .arg("-n")
        .arg(&installed)
        .status()
        .context("starting the new version")?;
    Ok(())
}

/// Exchanges two paths in one step, so there is no moment when Ticketsmith is
/// not installed and no way to end up with half of each version.
#[cfg(target_os = "macos")]
fn swap(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated strings that outlive the call.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) };
    if result != 0 {
        bail!("could not put the new version in place: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn swap(_: &Path, _: &Path) -> Result<()> {
    bail!("updates install only on macOS")
}

/// The `.app` this process is running out of. `Err` for `cargo run`, which
/// is a bare executable in `target/` with nothing an update could replace.
fn installed_bundle() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running executable")?;
    bundle_for(&exe).context("Ticketsmith is not running from an app bundle")
}

/// The `.app` an executable belongs to: three levels up, and a directory that
/// is a bundle rather than merely named like one.
fn bundle_for(exe: &Path) -> Option<PathBuf> {
    // Contents/MacOS/ticketsmith -> Contents/MacOS -> Contents -> Ticketsmith.app
    exe.parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|e| e == "app"))
        .filter(|path| path.join("Contents/Info.plist").is_file())
        .map(Path::to_path_buf)
}

/// Whether this copy can replace itself: it is an app bundle, in a folder
/// this user can write to. Not, then, a `cargo run` build, a copy run straight
/// from the disk image, or one macOS has translocated to a read-only path.
pub fn can_install() -> bool {
    installed_bundle()
        .ok()
        .and_then(|app| app.parent().map(writable))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let Ok(dir) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a NUL-terminated string that outlives the call.
    unsafe { libc::access(dir.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(target_os = "macos"))]
fn writable(_: &Path) -> bool {
    false
}

fn user_agent() -> String {
    format!("ticketsmith/{}", env!("CARGO_PKG_VERSION"))
}

/// Runs a checking tool, turning a non-zero exit into the sentence to show.
fn run(program: &str, args: &[&str], app: &Path, failure: &str) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .arg(app)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!("{failure}: {}", trimmed(&output.stderr));
    }
    Ok(())
}

/// The first line of a tool's output, for putting in a message.
fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or_default().trim().to_string()
}

// ---------------------------------------------------------------------------
// What the status bar shows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Nothing to offer: not checked yet, already current, or the check
    /// failed, which is not worth interrupting anyone over.
    Idle,
    Available(Release),
    Downloading(Release),
    /// Downloaded, mounted and proven ours. Installing restarts the app, which
    /// clears the ticket on screen, so it waits for its own click.
    Ready(Release),
    Installing(Release),
}

pub struct Updater {
    pub stage: Stage,
    /// The verified image, kept mounted between [`Stage::Ready`] and the click
    /// that installs it. Dropping it unmounts the volume.
    pub mounted: Option<Mounted>,
    /// Decided once: it is a couple of filesystem calls that cannot change
    /// while the app runs.
    pub installable: bool,
}

impl Updater {
    pub fn new() -> Self {
        Self { stage: Stage::Idle, mounted: None, installable: can_install() }
    }

    /// Takes a release a check found, unless an update is already under way.
    pub fn offer(&mut self, release: Release) {
        if matches!(self.stage, Stage::Idle | Stage::Available(_)) {
            self.stage = Stage::Available(release);
        }
    }

    /// The link in the status bar, when there is something to click.
    pub fn link(&self) -> Option<String> {
        match &self.stage {
            // A copy that cannot replace itself is still told about the
            // release; the link opens the release page instead.
            Stage::Available(r) if !self.installable => Some(format!("Download {}", r.version)),
            Stage::Available(r) => Some(format!("Update to {}", r.version)),
            Stage::Ready(_) => Some("Restart to update".into()),
            _ => None,
        }
    }

    /// What the status bar says beside the version while an update runs.
    pub fn progress(&self) -> Option<String> {
        match &self.stage {
            Stage::Downloading(r) => Some(format!("Downloading {}\u{2026}", r.version)),
            Stage::Installing(r) => Some(format!("Installing {}\u{2026}", r.version)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_with_or_without_their_v() {
        assert_eq!(Version::parse("0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse(" v1.10.3 "), Some(Version(1, 10, 3)));
        assert_eq!(Version::parse("v1"), Some(Version(1, 0, 0)));
        assert_eq!(Version::parse("v0.3.0-beta.1"), Some(Version(0, 3, 0)));
        assert_eq!(Version::parse("nightly"), None);
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
    }

    #[test]
    fn versions_order_by_number_and_not_by_text() {
        // "0.10.0" sorts before "0.9.0" as text, which would stop updates at
        // the tenth minor release.
        assert!(Version(0, 10, 0) > Version(0, 9, 0));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert!(Version(0, 2, 10) > Version(0, 2, 9));
    }

    #[test]
    fn the_status_curl_appends_is_split_from_the_body() {
        assert_eq!(split_status(b"{\"a\":1}\n200"), Some((&b"{\"a\":1}"[..], 200)));
        // A body with newlines of its own: the status is after the last one.
        assert_eq!(split_status(b"{\n}\n404"), Some((&b"{\n}"[..], 404)));
        assert_eq!(split_status(b"no status"), None);
    }

    /// A release payload shaped like the one GitHub returns.
    fn release_json(tag: &str, extra: &str) -> serde_json::Value {
        serde_json::from_str(&format!(
            r#"{{
                "tag_name": "{tag}",
                "html_url": "https://github.com/wes/ticketsmith/releases/tag/{tag}",
                {extra}
                "assets": [
                    {{ "browser_download_url": "https://example.test/Ticketsmith-9.0.0.dmg.sha256" }},
                    {{ "browser_download_url": "https://example.test/Ticketsmith-9.0.0.dmg" }}
                ]
            }}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_newer_release_is_offered_with_its_disk_image() {
        let release = latest_from(&release_json("v99.0.0", "")).unwrap().expect("newer");
        assert_eq!(release.version, Version(99, 0, 0));
        // Picking by suffix means the suffix, not containing it.
        assert_eq!(release.dmg_url, "https://example.test/Ticketsmith-9.0.0.dmg");
        assert_eq!(release.page_url, "https://github.com/wes/ticketsmith/releases/tag/v99.0.0");
    }

    #[test]
    fn the_version_already_running_is_not_an_update() {
        let same = format!("v{}", current());
        assert_eq!(latest_from(&release_json(&same, "")).unwrap(), None);
        assert_eq!(latest_from(&release_json("v0.0.1", "")).unwrap(), None);
    }

    #[test]
    fn a_draft_or_a_prerelease_is_left_alone() {
        assert_eq!(latest_from(&release_json("v99.0.0", r#""draft": true,"#)).unwrap(), None);
        assert_eq!(latest_from(&release_json("v99.0.0", r#""prerelease": true,"#)).unwrap(), None);
    }

    #[test]
    fn a_release_with_nothing_to_download_is_an_error_not_an_update() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "tag_name": "v99.0.0", "assets": [] }"#).unwrap();
        assert!(latest_from(&json).is_err());
    }

    fn release() -> Release {
        Release {
            version: Version(9, 9, 9),
            dmg_url: "https://example.test/Ticketsmith.dmg".into(),
            page_url: "https://example.test/release".into(),
        }
    }

    #[test]
    fn the_status_bar_offers_a_link_only_when_there_is_something_to_click() {
        let mut updater = Updater { installable: true, ..Updater::new() };
        assert_eq!(updater.link(), None);

        updater.offer(release());
        assert_eq!(updater.link().as_deref(), Some("Update to 9.9.9"));

        updater.stage = Stage::Downloading(release());
        assert_eq!(updater.link(), None, "a second click would start a second download");
        assert_eq!(updater.progress().as_deref(), Some("Downloading 9.9.9\u{2026}"));

        updater.stage = Stage::Ready(release());
        assert_eq!(updater.link().as_deref(), Some("Restart to update"));

        updater.stage = Stage::Installing(release());
        assert_eq!(updater.link(), None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_copy_that_cannot_replace_itself_links_to_the_release_page() {
        // This test binary is not an app bundle.
        let mut updater = Updater::new();
        assert!(!updater.installable);
        updater.offer(release());
        assert_eq!(updater.link().as_deref(), Some("Download 9.9.9"));
    }

    #[test]
    fn a_later_check_does_not_interrupt_an_update_under_way() {
        let mut updater = Updater { installable: true, ..Updater::new() };
        updater.stage = Stage::Ready(release());
        updater.offer(Release { version: Version(10, 0, 0), ..release() });
        assert_eq!(updater.stage, Stage::Ready(release()));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_team_requirement_is_one_codesign_can_read() {
        // A requirement that does not compile fails every verification with
        // the same exit as a foreign signature, so no update would ever install.
        let out = std::env::temp_dir().join(format!("ticketsmith-req-{}", std::process::id()));
        let compiled = Command::new("/usr/bin/csreq")
            .arg("-r")
            .arg(requirement())
            .arg("-b")
            .arg(&out)
            .output()
            .unwrap();
        std::fs::remove_file(&out).ok();
        assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_test_binary_is_not_an_installed_bundle() {
        assert!(installed_bundle().is_err());
        assert!(!can_install());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_bundled_executable_finds_the_app_around_it() {
        let dir = std::env::temp_dir().join(format!("ticketsmith-bundle-{}", std::process::id()));
        let app = dir.join("Ticketsmith.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), "<plist/>").unwrap();
        let exe = app.join("Contents/MacOS/ticketsmith");

        assert_eq!(bundle_for(&exe).as_deref(), Some(app.as_path()));
        assert!(writable(&dir));

        // Named like a bundle, but without the file that makes it one.
        std::fs::remove_file(app.join("Contents/Info.plist")).unwrap();
        assert_eq!(bundle_for(&exe), None);
        assert_eq!(bundle_for(Path::new("/Users/x/ticketsmith/target/debug/ticketsmith")), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_foreign_signature_is_refused() {
        // An ad-hoc signed bundle: intact, but not from the team.
        let dir = std::env::temp_dir().join(format!("ticketsmith-foreign-{}", std::process::id()));
        let app = dir.join("Ticketsmith.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/ticketsmith")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleExecutable</key><string>ticketsmith</string>\
             <key>CFBundleIdentifier</key><string>com.joedesigns.ticketsmith</string>\
             </dict></plist>",
        )
        .unwrap();
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .status()
            .unwrap();
        assert!(signed.success());

        run("/usr/bin/codesign", &["--verify", "--deep", "--strict"], &app, "intact").unwrap();
        let err = run("/usr/bin/codesign", &["--verify", "-R", &requirement()], &app, "foreign")
            .unwrap_err();
        assert!(err.to_string().starts_with("foreign"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
