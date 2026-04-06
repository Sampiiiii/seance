use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{artifacts, changelog, sparkle, workspace};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub tag_name: String,
    pub release_title: String,
    pub repository_slug: String,
    pub release_dir: String,
    pub release_notes_path: String,
    pub checksums_path: String,
    pub appcast: AppcastPlan,
    pub platforms: Vec<PlatformPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppcastPlan {
    pub sparkle_item_path: String,
    pub output_path: String,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformPlan {
    pub platform: String,
    pub arch: String,
    pub manifest_path: String,
    pub artifacts: Vec<ArtifactPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPlan {
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformManifest {
    pub platform: String,
    pub arch: String,
    pub artifacts: Vec<ArtifactPlan>,
}

pub fn prepare_release(
    tag_ref: &str,
    release_dir: &Path,
    manifest_out: &Path,
    release_notes_out: &Path,
) -> Result<()> {
    let tag_name = normalize_tag_name(tag_ref);
    let version = workspace::verified_package_version("seance-app", Some(tag_version(&tag_name)?))?;
    let release_notes = changelog::release_notes(Path::new("CHANGELOG.md"), &version)?;
    let repository_slug = repository_slug()?;

    write_string(release_notes_out, &(release_notes + "\n"))?;

    let release_dir_string = release_dir.display().to_string();
    let manifest = ReleaseManifest {
        version: version.clone(),
        release_title: format!("Séance v{version}"),
        tag_name: tag_name.clone(),
        repository_slug: repository_slug.clone(),
        release_dir: release_dir_string.clone(),
        release_notes_path: release_notes_out.display().to_string(),
        checksums_path: release_dir
            .join(artifacts::artifact_name("checksums", None)?)
            .display()
            .to_string(),
        appcast: AppcastPlan {
            sparkle_item_path: release_dir
                .join(artifacts::artifact_name("sparkle-item", None)?)
                .display()
                .to_string(),
            output_path: Path::new("site/sparkle/stable/appcast.xml")
                .display()
                .to_string(),
            download_url: format!(
                "https://github.com/{repository_slug}/releases/download/{tag_name}/{}",
                artifacts::artifact_name("macos-app-zip", None)?
            ),
        },
        platforms: vec![
            platform_plan(&release_dir_string, "macos", "aarch64")?,
            platform_plan(&release_dir_string, "linux", "x86_64")?,
            platform_plan(&release_dir_string, "linux", "aarch64")?,
        ],
    };

    write_json(manifest_out, &manifest)
}

pub fn ensure_draft_release(manifest_path: &Path) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    let status = Command::new("gh")
        .args(["release", "view", &manifest.tag_name])
        .status()
        .context("failed to execute gh release view")?;
    if status.success() {
        return Ok(());
    }

    let create_status = Command::new("gh")
        .args([
            "release",
            "create",
            &manifest.tag_name,
            "--draft",
            "--title",
            &manifest.release_title,
            "--notes-file",
            &manifest.release_notes_path,
        ])
        .status()
        .context("failed to execute gh release create")?;
    if !create_status.success() {
        bail!("gh release create exited with status {create_status}");
    }

    Ok(())
}

pub fn write_platform_manifest(
    manifest_path: &Path,
    platform: &str,
    arch: &str,
    artifact_paths: &[PathBuf],
) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    let plan = manifest.platform(platform, arch)?;

    let planned_names = plan
        .artifacts
        .iter()
        .map(|artifact| file_name(Path::new(&artifact.path)))
        .collect::<Result<Vec<_>>>()?;
    let actual_names = artifact_paths
        .iter()
        .map(|path| file_name(path))
        .collect::<Result<Vec<_>>>()?;

    for planned in &planned_names {
        if !actual_names.iter().any(|actual| actual == planned) {
            bail!("missing expected artifact {planned} for {platform}-{arch}");
        }
    }

    let platform_manifest = PlatformManifest {
        platform: platform.to_owned(),
        arch: arch.to_owned(),
        artifacts: artifact_paths
            .iter()
            .map(|path| {
                let name = file_name(path)?;
                Ok(ArtifactPlan {
                    kind: artifact_kind_for_name(&name)?.to_owned(),
                    path: path.display().to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };

    write_json(Path::new(&plan.manifest_path), &platform_manifest)
}

pub fn upload_platform_assets(manifest_path: &Path, platform: &str, arch: &str) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    let plan = manifest.platform(platform, arch)?;
    let platform_manifest = read_platform_manifest(Path::new(&plan.manifest_path))?;

    let mut command = Command::new("gh");
    command.args(["release", "upload", &manifest.tag_name]);
    for artifact in &platform_manifest.artifacts {
        command.arg(&artifact.path);
    }
    command.arg("--clobber");

    let status = command
        .status()
        .context("failed to execute gh release upload")?;
    if !status.success() {
        bail!("gh release upload exited with status {status}");
    }

    Ok(())
}

pub fn validate_from_manifest(manifest_path: &Path) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    sparkle::validate_artifacts(
        Path::new(&manifest.release_dir),
        &manifest
            .platforms
            .iter()
            .flat_map(|platform| platform.artifacts.iter())
            .map(|artifact| file_name(Path::new(&artifact.path)))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let sparkle_item = file_name(Path::new(&manifest.appcast.sparkle_item_path))?;
    sparkle::validate_artifacts(Path::new(&manifest.release_dir), &[sparkle_item])?;

    for platform in &manifest.platforms {
        if !Path::new(&platform.manifest_path).exists() {
            bail!("missing platform manifest {}", platform.manifest_path);
        }
    }

    Ok(())
}

pub fn write_checksums_from_manifest(manifest_path: &Path) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    artifacts::write_checksums(
        Path::new(&manifest.release_dir),
        Path::new(&manifest.checksums_path),
    )
}

pub fn generate_appcast_from_manifest(manifest_path: &Path) -> Result<()> {
    let manifest = read_release_manifest(manifest_path)?;
    sparkle::write_appcast(
        Path::new(&manifest.appcast.sparkle_item_path),
        &manifest.appcast.download_url,
        None,
        Path::new(&manifest.appcast.output_path),
    )
}

pub fn read_release_manifest(path: &Path) -> Result<ReleaseManifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read release manifest at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse release manifest at {}", path.display()))
}

fn read_platform_manifest(path: &Path) -> Result<PlatformManifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read platform manifest at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse platform manifest at {}", path.display()))
}

impl ReleaseManifest {
    fn platform(&self, platform: &str, arch: &str) -> Result<&PlatformPlan> {
        self.platforms
            .iter()
            .find(|entry| entry.platform == platform && entry.arch == arch)
            .ok_or_else(|| anyhow!("platform plan {platform}-{arch} not found in release manifest"))
    }
}

fn platform_plan(release_dir: &str, platform: &str, arch: &str) -> Result<PlatformPlan> {
    let mut artifacts = Vec::new();
    match (platform, arch) {
        ("macos", "aarch64") => {
            artifacts.push(artifact_plan("macos-dmg", release_dir, None)?);
            artifacts.push(artifact_plan("macos-app-zip", release_dir, None)?);
            artifacts.push(artifact_plan("sparkle-item", release_dir, None)?);
        }
        ("linux", "x86_64") | ("linux", "aarch64") => {
            artifacts.push(artifact_plan("linux-appimage", release_dir, Some(arch))?);
            artifacts.push(artifact_plan("linux-zsync", release_dir, Some(arch))?);
        }
        _ => bail!("unsupported platform plan {platform}-{arch}"),
    }

    Ok(PlatformPlan {
        platform: platform.to_owned(),
        arch: arch.to_owned(),
        manifest_path: Path::new(release_dir)
            .join("manifests")
            .join(format!("{platform}-{arch}.json"))
            .display()
            .to_string(),
        artifacts,
    })
}

fn artifact_plan(kind: &str, release_dir: &str, arch: Option<&str>) -> Result<ArtifactPlan> {
    let name = artifacts::artifact_name(kind, arch)?;
    Ok(ArtifactPlan {
        kind: kind.to_owned(),
        path: Path::new(release_dir).join(name).display().to_string(),
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path {} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;
    let json = serde_json::to_string_pretty(value).context("failed to serialize JSON")?;
    fs::write(path, format!("{json}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn write_string(path: &Path, value: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path {} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;
    fs::write(path, value).with_context(|| format!("failed to write {}", path.display()))
}

fn normalize_tag_name(tag_ref: &str) -> String {
    tag_ref.trim().trim_start_matches("refs/tags/").to_owned()
}

fn tag_version(tag_name: &str) -> Result<&str> {
    let version = tag_name.trim_start_matches('v');
    if version.is_empty() {
        bail!("tag {tag_name} does not contain a version");
    }
    Ok(version)
}

fn repository_slug() -> Result<String> {
    let repo = env!("CARGO_PKG_REPOSITORY").trim_end_matches(".git");
    if let Some(rest) = repo.strip_prefix("https://github.com/") {
        return Ok(rest.to_owned());
    }
    if let Some(rest) = repo.strip_prefix("git@github.com:") {
        return Ok(rest.to_owned());
    }
    bail!("repository must be a GitHub repository URL")
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("path {} does not have a valid file name", path.display()))
}

fn artifact_kind_for_name(name: &str) -> Result<&'static str> {
    match name {
        "seance-macos-aarch64.dmg" => Ok("macos-dmg"),
        "seance-macos-aarch64.app.zip" => Ok("macos-app-zip"),
        "sparkle-item.json" => Ok("sparkle-item"),
        other if other.ends_with(".AppImage") => Ok("linux-appimage"),
        other if other.ends_with(".AppImage.zsync") => Ok("linux-zsync"),
        other => bail!("unsupported artifact file name {other}"),
    }
}
