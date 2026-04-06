use std::{
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::Path,
};

use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};

const MACOS_APP_ZIP: &str = "seance-macos-aarch64.app.zip";
const MACOS_DMG: &str = "seance-macos-aarch64.dmg";
const SPARKLE_ITEM: &str = "sparkle-item.json";
const CHECKSUMS: &str = "SHA256SUMS.txt";
const SUPPORTED_LINUX_ARCHES: [&str; 2] = ["x86_64", "aarch64"];

pub fn artifact_name(kind: &str, arch: Option<&str>) -> Result<String> {
    match kind {
        "macos-app-zip" => Ok(MACOS_APP_ZIP.to_owned()),
        "macos-dmg" => Ok(MACOS_DMG.to_owned()),
        "sparkle-item" => Ok(SPARKLE_ITEM.to_owned()),
        "checksums" => Ok(CHECKSUMS.to_owned()),
        "linux-appimage" => {
            let arch = require_linux_arch(arch)?;
            Ok(format!("seance-linux-{arch}.AppImage"))
        }
        "linux-zsync" => {
            let appimage = artifact_name("linux-appimage", arch)?;
            Ok(format!("{appimage}.zsync"))
        }
        other => bail!("unsupported artifact kind {other}"),
    }
}

pub fn linux_target_triple(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => bail!("unsupported linux architecture {other}"),
    }
}

pub fn linux_update_information(repo_slug: &str, arch: &str) -> Result<String> {
    let (owner, repo) = parse_repo_slug(repo_slug)?;
    let zsync = artifact_name("linux-zsync", Some(arch))?;
    Ok(format!("gh-releases-zsync|{owner}|{repo}|latest|{zsync}"))
}

pub fn release_artifacts(include_metadata: bool) -> Vec<String> {
    let mut artifacts = vec![
        artifact_name("macos-dmg", None).expect("static macOS dmg artifact name"),
        artifact_name("macos-app-zip", None).expect("static macOS app zip artifact name"),
    ];

    for arch in SUPPORTED_LINUX_ARCHES {
        artifacts.push(
            artifact_name("linux-appimage", Some(arch))
                .expect("static Linux AppImage artifact name"),
        );
        artifacts.push(
            artifact_name("linux-zsync", Some(arch)).expect("static Linux zsync artifact name"),
        );
    }

    if include_metadata {
        artifacts.push(artifact_name("sparkle-item", None).expect("static sparkle metadata name"));
    }

    artifacts
}

pub fn validate_release_dir(release_dir: &Path, include_metadata: bool) -> Result<()> {
    crate::sparkle::validate_artifacts(release_dir, &release_artifacts(include_metadata))
}

pub fn artifact_paths(group: &str, release_dir: &Path) -> Result<Vec<String>> {
    let names = match group {
        "macos-release" => vec![
            artifact_name("macos-dmg", None)?,
            artifact_name("macos-app-zip", None)?,
            artifact_name("sparkle-item", None)?,
        ],
        other => bail!("unsupported artifact path group {other}"),
    };

    Ok(names
        .into_iter()
        .map(|name| release_dir.join(name).display().to_string())
        .collect())
}

pub fn write_checksums(release_dir: &Path, output_path: &Path) -> Result<()> {
    let parent = output_path
        .parent()
        .ok_or_else(|| anyhow!("output path must have a parent directory"))?;
    fs::create_dir_all(parent)?;

    let mut writer = BufWriter::new(File::create(output_path)?);
    for artifact in release_artifacts(false) {
        let digest = sha256_file(&release_dir.join(&artifact))?;
        writeln!(writer, "{digest}  {artifact}")?;
    }
    writer.flush()?;
    Ok(())
}

fn require_linux_arch(arch: Option<&str>) -> Result<&str> {
    let arch = arch.ok_or_else(|| anyhow!("--arch is required for linux artifact kinds"))?;
    linux_target_triple(arch)?;
    Ok(arch)
}

fn parse_repo_slug(repo_slug: &str) -> Result<(&str, &str)> {
    let (owner, repo) = repo_slug
        .split_once('/')
        .ok_or_else(|| anyhow!("repo slug must be in <owner>/<repo> form"))?;
    if owner.is_empty() || repo.is_empty() {
        bail!("repo slug must be in <owner>/<repo> form");
    }
    Ok((owner, repo))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
