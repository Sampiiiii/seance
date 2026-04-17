use std::{
    env,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

#[cfg(target_os = "linux")]
use std::{path::PathBuf, process::Command};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use semver::Version;
#[cfg(target_os = "linux")]
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallMode {
    Prompted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub install_mode: InstallMode,
    pub channel: ReleaseChannel,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            install_mode: InstallMode::Prompted,
            channel: ReleaseChannel::Stable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub published_at: Option<String>,
    pub notes_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available(UpdateInfo),
    Downloading,
    Installing,
    ReadyToRelaunch,
    UpToDate,
    Failed(String),
}

pub trait UpdateBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn support_status(&self) -> Result<()>;
    fn check(&self, current_version: &Version, settings: &UpdateSettings) -> Result<UpdateCheck>;
    fn install(
        &self,
        update: &UpdateInfo,
        settings: &UpdateSettings,
        publish: &mut dyn FnMut(UpdateState),
    ) -> Result<()>;
}

#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<UpdateManagerInner>,
}

struct UpdateManagerInner {
    backend: Arc<dyn UpdateBackend>,
    current_version: Version,
    settings: Mutex<UpdateSettings>,
    state: Mutex<UpdateState>,
    subscribers: Mutex<Vec<Sender<UpdateState>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available(UpdateInfo),
}

#[cfg(target_os = "macos")]
mod sparkle_bridge {
    use anyhow::{Result, anyhow};
    use libc::{RTLD_DEFAULT, c_char, c_void, dlsym};
    use std::ffi::CString;

    type InitFn = unsafe extern "C" fn(*const c_char) -> bool;
    type CheckFn = unsafe extern "C" fn() -> bool;

    fn resolve<T>(name: &[u8]) -> Option<T>
    where
        T: Copy,
    {
        let symbol = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr().cast::<c_char>()) };
        if symbol.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&symbol) })
        }
    }

    pub(super) fn initialize(feed_url: &str) -> Result<()> {
        let feed_url = CString::new(feed_url)
            .map_err(|_| anyhow!("Sparkle feed URL contains an interior nul byte"))?;
        let Some(init) = resolve::<InitFn>(b"seance_sparkle_initialize\0") else {
            return Err(anyhow!(
                "Sparkle.framework bridge is unavailable in this process"
            ));
        };
        let ok = unsafe { init(feed_url.as_ptr()) };
        if ok {
            Ok(())
        } else {
            Err(anyhow!(
                "Sparkle.framework is unavailable or failed to initialize"
            ))
        }
    }

    pub(super) fn check_for_updates() -> Result<()> {
        let Some(check) = resolve::<CheckFn>(b"seance_sparkle_check_for_updates\0") else {
            return Err(anyhow!(
                "Sparkle.framework bridge is unavailable in this process"
            ));
        };
        let ok = unsafe { check() };
        if ok {
            Ok(())
        } else {
            Err(anyhow!(
                "Sparkle.framework is unavailable or failed to start updating"
            ))
        }
    }
}

impl UpdateManager {
    pub fn new(settings: UpdateSettings) -> Self {
        let current_version = Version::parse(env!("CARGO_PKG_VERSION")).expect("valid version");
        Self {
            inner: Arc::new(UpdateManagerInner {
                backend: default_backend(),
                current_version,
                settings: Mutex::new(settings),
                state: Mutex::new(UpdateState::Idle),
                subscribers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn current_version(&self) -> String {
        self.inner.current_version.to_string()
    }

    pub fn settings_snapshot(&self) -> UpdateSettings {
        self.inner
            .settings
            .lock()
            .expect("update settings mutex poisoned")
            .clone()
    }

    pub fn update_settings(&self, settings: UpdateSettings) {
        *self
            .inner
            .settings
            .lock()
            .expect("update settings mutex poisoned") = settings;
    }

    pub fn state_snapshot(&self) -> UpdateState {
        self.inner
            .state
            .lock()
            .expect("update state mutex poisoned")
            .clone()
    }

    pub fn subscribe(&self) -> Receiver<UpdateState> {
        let (tx, rx) = mpsc::channel();
        let snapshot = self.state_snapshot();
        let _ = tx.send(snapshot);
        self.inner
            .subscribers
            .lock()
            .expect("update subscriber mutex poisoned")
            .push(tx);
        rx
    }

    pub fn startup_check(&self) {
        if cfg!(test) {
            return;
        }
        let settings = self.settings_snapshot();
        if settings.auto_check {
            self.check_internal(true);
        }
    }

    pub fn check_now(&self) {
        self.check_internal(false);
    }

    pub fn install_update(&self) {
        let update = match self.state_snapshot() {
            UpdateState::Available(update) => update,
            UpdateState::ReadyToRelaunch => return,
            _ => {
                self.publish(UpdateState::Failed(
                    "No downloaded update is ready to install.".into(),
                ));
                return;
            }
        };

        let backend = Arc::clone(&self.inner.backend);
        let settings = self.settings_snapshot();
        let manager = self.clone();
        thread::spawn(move || {
            manager.publish(UpdateState::Downloading);
            let mut publish = |state| manager.publish(state);
            match backend.install(&update, &settings, &mut publish) {
                Ok(()) => {
                    if !matches!(manager.state_snapshot(), UpdateState::ReadyToRelaunch) {
                        manager.publish(UpdateState::ReadyToRelaunch);
                    }
                }
                Err(error) => manager.publish(UpdateState::Failed(error.to_string())),
            }
        });
    }

    pub fn dismiss_update(&self) {
        self.publish(UpdateState::Idle);
    }

    fn check_internal(&self, silent: bool) {
        let backend = Arc::clone(&self.inner.backend);
        let current_version = self.inner.current_version.clone();
        let settings = self.settings_snapshot();
        let manager = self.clone();

        thread::spawn(move || {
            if silent && !settings.auto_check {
                return;
            }

            manager.publish(UpdateState::Checking);
            match backend.check(&current_version, &settings) {
                Ok(UpdateCheck::UpToDate) => manager.publish(UpdateState::UpToDate),
                Ok(UpdateCheck::Available(update)) => {
                    manager.publish(UpdateState::Available(update))
                }
                Err(error) if silent => {
                    tracing::debug!(error = %error, backend = backend.name(), "silent update check failed");
                    manager.publish(UpdateState::Idle);
                }
                Err(error) => manager.publish(UpdateState::Failed(error.to_string())),
            }
        });
    }

    fn publish(&self, state: UpdateState) {
        *self
            .inner
            .state
            .lock()
            .expect("update state mutex poisoned") = state.clone();
        self.inner
            .subscribers
            .lock()
            .expect("update subscriber mutex poisoned")
            .retain(|subscriber| subscriber.send(state.clone()).is_ok());
    }
}

fn default_backend() -> Arc<dyn UpdateBackend> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(MacosUpdateBackend::new())
    }

    #[cfg(target_os = "linux")]
    {
        Arc::new(LinuxUpdateBackend::new())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Arc::new(UnsupportedUpdateBackend::new(
            "In-app updates are only implemented for macOS and Linux builds.",
        ))
    }
}

fn github_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(format!("seance-updater/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to construct update client")
}

fn repository_slug() -> Result<(String, String)> {
    let raw = env!("CARGO_PKG_REPOSITORY").trim_end_matches(".git");
    if let Some(rest) = raw.strip_prefix("https://github.com/") {
        let mut parts = rest.split('/');
        let owner = parts.next().unwrap_or_default();
        let repo = parts.next().unwrap_or_default();
        if !owner.is_empty() && !repo.is_empty() {
            return Ok((owner.to_string(), repo.to_string()));
        }
    }
    if let Some(rest) = raw.strip_prefix("git@github.com:") {
        let mut parts = rest.split('/');
        let owner = parts.next().unwrap_or_default();
        let repo = parts.next().unwrap_or_default();
        if !owner.is_empty() && !repo.is_empty() {
            return Ok((owner.to_string(), repo.to_string()));
        }
    }
    bail!("repository URL must point at a GitHub repository")
}

fn github_pages_feed_url() -> Result<String> {
    if let Ok(value) = env::var("SEANCE_SPARKLE_FEED_URL")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    let (owner, repo) = repository_slug()?;
    Ok(format!(
        "https://{}.github.io/{}/sparkle/stable/appcast.xml",
        owner.to_lowercase(),
        repo
    ))
}

fn parse_release_version(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).with_context(|| format!("failed to parse version '{raw}'"))
}

#[cfg(target_os = "linux")]
fn asset_arch_suffix() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Deserialize)]
struct GithubLatestRelease {
    tag_name: String,
    html_url: Option<String>,
    published_at: Option<String>,
    assets: Vec<GithubReleaseAsset>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(target_os = "linux")]
struct LinuxUpdateBackend;

#[cfg(target_os = "linux")]
impl LinuxUpdateBackend {
    fn new() -> Self {
        Self
    }

    fn latest_release(&self) -> Result<GithubLatestRelease> {
        let (owner, repo) = repository_slug()?;
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        github_client()?
            .get(url)
            .send()
            .context("failed to fetch latest GitHub release")?
            .error_for_status()
            .context("GitHub latest release request failed")?
            .json::<GithubLatestRelease>()
            .context("failed to parse latest GitHub release response")
    }

    fn ensure_appimage_install(&self) -> Result<PathBuf> {
        let appimage = env::var_os("APPIMAGE")
            .ok_or_else(|| anyhow!("Updates are available only for AppImage installs."))?;
        Ok(PathBuf::from(appimage))
    }

    fn appimage_update_binary(&self) -> Result<&'static str> {
        for candidate in ["AppImageUpdate", "appimageupdatetool"] {
            if Command::new(candidate).arg("--help").output().is_ok() {
                return Ok(candidate);
            }
        }
        bail!("AppImageUpdate is not available in this runtime.")
    }
}

#[cfg(target_os = "linux")]
impl UpdateBackend for LinuxUpdateBackend {
    fn name(&self) -> &'static str {
        "appimage"
    }

    fn support_status(&self) -> Result<()> {
        self.ensure_appimage_install().map(|_| ())
    }

    fn check(&self, current_version: &Version, _settings: &UpdateSettings) -> Result<UpdateCheck> {
        self.support_status()?;
        let release = self.latest_release()?;
        let latest_version = parse_release_version(&release.tag_name)?;
        let arch = asset_arch_suffix();
        let expected_asset = format!("seance-linux-{arch}.AppImage.zsync");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == expected_asset)
            .ok_or_else(|| anyhow!("latest release is missing {expected_asset}"))?;

        if latest_version <= *current_version {
            return Ok(UpdateCheck::UpToDate);
        }

        Ok(UpdateCheck::Available(UpdateInfo {
            version: latest_version.to_string(),
            published_at: release.published_at,
            notes_url: release
                .html_url
                .or_else(|| Some(asset.browser_download_url.clone())),
        }))
    }

    fn install(
        &self,
        _update: &UpdateInfo,
        _settings: &UpdateSettings,
        publish: &mut dyn FnMut(UpdateState),
    ) -> Result<()> {
        let appimage = self.ensure_appimage_install()?;
        let updater = self.appimage_update_binary()?;

        publish(UpdateState::Installing);
        let status = Command::new(updater)
            .arg(&appimage)
            .status()
            .with_context(|| format!("failed to launch {updater}"))?;
        if !status.success() {
            bail!("{updater} exited with status {status}");
        }

        publish(UpdateState::ReadyToRelaunch);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
struct MacosUpdateBackend {
    feed_url: String,
}

#[cfg(target_os = "macos")]
impl MacosUpdateBackend {
    fn new() -> Self {
        let feed_url = github_pages_feed_url()
            .unwrap_or_else(|_| "https://example.invalid/appcast.xml".into());
        Self { feed_url }
    }

    fn latest_release(&self) -> Result<UpdateInfo> {
        let xml = github_client()?
            .get(&self.feed_url)
            .send()
            .with_context(|| format!("failed to fetch appcast {}", self.feed_url))?
            .error_for_status()
            .context("Sparkle appcast request failed")?
            .text()
            .context("failed to read Sparkle appcast")?;

        let doc =
            roxmltree::Document::parse(&xml).context("failed to parse Sparkle appcast XML")?;
        let item = doc
            .descendants()
            .find(|node| node.has_tag_name("item"))
            .ok_or_else(|| anyhow!("Sparkle appcast is missing an <item>"))?;

        let enclosure = item
            .children()
            .find(|node| node.has_tag_name("enclosure"))
            .ok_or_else(|| anyhow!("Sparkle appcast item is missing an <enclosure>"))?;

        let version = enclosure
            .attribute("sparkle:shortVersionString")
            .or_else(|| enclosure.attribute("shortVersionString"))
            .or_else(|| {
                item.children()
                    .find(|node| node.tag_name().name() == "shortVersionString")
                    .and_then(|node| node.text())
            })
            .ok_or_else(|| anyhow!("Sparkle appcast item is missing sparkle:shortVersionString"))?;

        let notes_url = item
            .children()
            .find(|node| node.tag_name().name() == "releaseNotesLink")
            .and_then(|node| node.text())
            .map(ToOwned::to_owned);

        let published_at = item
            .children()
            .find(|node| node.has_tag_name("pubDate"))
            .and_then(|node| node.text())
            .map(ToOwned::to_owned);

        Ok(UpdateInfo {
            version: version.to_string(),
            published_at,
            notes_url,
        })
    }
}

#[cfg(target_os = "macos")]
impl UpdateBackend for MacosUpdateBackend {
    fn name(&self) -> &'static str {
        "sparkle"
    }

    fn support_status(&self) -> Result<()> {
        sparkle_bridge::initialize(&self.feed_url)
    }

    fn check(&self, current_version: &Version, _settings: &UpdateSettings) -> Result<UpdateCheck> {
        self.support_status()?;
        let release = self.latest_release()?;
        let latest_version = parse_release_version(&release.version)?;
        if latest_version <= *current_version {
            return Ok(UpdateCheck::UpToDate);
        }
        Ok(UpdateCheck::Available(release))
    }

    fn install(
        &self,
        _update: &UpdateInfo,
        _settings: &UpdateSettings,
        publish: &mut dyn FnMut(UpdateState),
    ) -> Result<()> {
        publish(UpdateState::Installing);
        sparkle_bridge::check_for_updates()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
struct UnsupportedUpdateBackend {
    message: &'static str,
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl UnsupportedUpdateBackend {
    fn new(message: &'static str) -> Self {
        Self { message }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl UpdateBackend for UnsupportedUpdateBackend {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn support_status(&self) -> Result<()> {
        bail!(self.message)
    }

    fn check(&self, _current_version: &Version, _settings: &UpdateSettings) -> Result<UpdateCheck> {
        self.support_status()?;
        Ok(UpdateCheck::UpToDate)
    }

    fn install(
        &self,
        _update: &UpdateInfo,
        _settings: &UpdateSettings,
        _publish: &mut dyn FnMut(UpdateState),
    ) -> Result<()> {
        self.support_status()
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateManager, UpdateSettings, parse_release_version, repository_slug};

    #[test]
    fn parses_workspace_repository_slug() {
        let (owner, repo) = repository_slug().unwrap();
        assert_eq!(owner, "Sampiiiii");
        assert_eq!(repo, "seance");
    }

    #[test]
    fn release_versions_accept_leading_v() {
        let version = parse_release_version("v1.2.3").unwrap();
        assert_eq!(version.to_string(), "1.2.3");
    }

    #[test]
    fn update_subscribers_receive_initial_state() {
        let manager = UpdateManager::new(UpdateSettings::default());
        let rx = manager.subscribe();
        assert_eq!(rx.recv().unwrap(), super::UpdateState::Idle);
    }
}
