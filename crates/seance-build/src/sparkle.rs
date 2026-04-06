use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct SparkleItem {
    pub version: String,
    pub signature: String,
    pub length: u64,
    pub pub_date: String,
}

pub fn write_sparkle_item(
    version: &str,
    artifact_path: &Path,
    output_path: &Path,
    sign_update_path: &Path,
    pub_date: Option<&str>,
) -> Result<()> {
    let signature = if sign_update_path.exists() {
        sign_artifact(sign_update_path, artifact_path)?
    } else {
        String::new()
    };
    let payload = SparkleItem {
        version: version.to_owned(),
        signature,
        length: fs::metadata(artifact_path)
            .with_context(|| format!("failed to read metadata for {}", artifact_path.display()))?
            .len(),
        pub_date: pub_date
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Utc::now().to_rfc2822()),
    };

    write_json(output_path, &payload)
}

pub fn read_sparkle_item(path: &Path) -> Result<SparkleItem> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read sparkle metadata at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse sparkle metadata at {}", path.display()))
}

pub fn write_appcast(
    item_path: &Path,
    download_url: &str,
    release_notes_url: Option<&str>,
    output_path: &Path,
) -> Result<()> {
    let item = read_sparkle_item(item_path)?;
    let notes_xml = release_notes_url
        .filter(|url| !url.is_empty())
        .map(|url| {
            format!(
                "    <sparkle:releaseNotesLink>{}</sparkle:releaseNotesLink>\n",
                escape_xml(url)
            )
        })
        .unwrap_or_default();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rss version=\"2.0\"\n     xmlns:sparkle=\"http://www.andymatuschak.org/xml-namespaces/sparkle\"\n     xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n  <channel>\n    <title>Séance Stable Updates</title>\n    <link>https://github.com/Sampiiiii/seance/releases</link>\n    <description>Stable update feed for Séance</description>\n    <language>en</language>\n    <item>\n      <title>Séance {version}</title>\n      <pubDate>{pub_date}</pubDate>\n{notes_xml}      <enclosure\n        url=\"{download_url}\"\n        sparkle:version=\"{version}\"\n        sparkle:shortVersionString=\"{version}\"\n        sparkle:edSignature=\"{signature}\"\n        length=\"{length}\"\n        type=\"application/octet-stream\" />\n    </item>\n  </channel>\n</rss>\n",
        version = escape_xml(&item.version),
        pub_date = escape_xml(&item.pub_date),
        notes_xml = notes_xml,
        download_url = escape_xml(download_url),
        signature = escape_xml(&item.signature),
        length = item.length,
    );

    if item.signature.is_empty() {
        eprintln!(
            "warning: sparkle signature is empty; appcast will be generated without edSignature"
        );
    }

    write_string(output_path, &xml)
}

pub fn validate_artifacts(release_dir: &Path, artifact_names: &[String]) -> Result<()> {
    let missing: Vec<PathBuf> = artifact_names
        .iter()
        .map(|artifact| release_dir.join(artifact))
        .filter(|path| !path.exists())
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let joined = missing
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("missing release artifacts: {joined}");
}

fn sign_artifact(sign_update_path: &Path, artifact_path: &Path) -> Result<String> {
    let output = Command::new(sign_update_path)
        .arg(artifact_path)
        .output()
        .with_context(|| {
            format!(
                "failed to execute {} for {}",
                sign_update_path.display(),
                artifact_path.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "{} exited with status {}",
            sign_update_path.display(),
            output.status
        );
    }

    String::from_utf8(output.stdout)
        .context("sign_update output was not valid UTF-8")
        .map(|output| output.trim().to_owned())
}

fn write_json<T: Serialize>(output_path: &Path, payload: &T) -> Result<()> {
    let content = serde_json::to_string_pretty(payload).context("failed to serialize JSON")?;
    write_string(output_path, &(content + "\n"))
}

fn write_string(output_path: &Path, content: &str) -> Result<()> {
    let parent = output_path.parent().with_context(|| {
        format!(
            "output path {} does not have a parent directory",
            output_path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;
    fs::write(output_path, content)
        .with_context(|| format!("failed to write {}", output_path.display()))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
