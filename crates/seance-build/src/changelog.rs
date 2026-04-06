use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

pub fn release_notes(changelog_path: &Path, version: &str) -> Result<String> {
    let content = fs::read_to_string(changelog_path)
        .with_context(|| format!("failed to read changelog at {}", changelog_path.display()))?;
    let heading = format!("## [{version}]");
    let mut section = Vec::new();
    let mut in_section = false;

    for line in content.lines() {
        if line.starts_with(&heading) {
            in_section = true;
            section.push(line.to_owned());
            continue;
        }

        if in_section && line.starts_with("## [") {
            break;
        }

        if in_section {
            section.push(line.to_owned());
        }
    }

    if section.is_empty() {
        bail!(
            "could not find release notes for version {version} in {}",
            changelog_path.display()
        );
    }

    Ok(section.join("\n"))
}
