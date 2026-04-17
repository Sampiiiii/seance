use anyhow::{Context, Result, bail};
use cargo_metadata::MetadataCommand;

pub(crate) fn package_version(package_name: &str) -> Result<String> {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to query cargo metadata")?;
    let package = metadata
        .packages
        .into_iter()
        .find(|package| package.name == package_name)
        .with_context(|| format!("package {package_name} was not found in cargo metadata"))?;

    Ok(package.version.to_string())
}

pub(crate) fn verified_package_version(
    package_name: &str,
    expected_tag: Option<&str>,
) -> Result<String> {
    let version = package_version(package_name)?;

    if let Some(tag) = expected_tag
        && version != tag
    {
        bail!("tag version {tag} does not match package {package_name} version {version}");
    }

    Ok(version)
}
