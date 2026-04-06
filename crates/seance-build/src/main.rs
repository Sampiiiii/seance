mod artifacts;
mod changelog;
mod manifest;
mod sparkle;
mod workspace;

use std::{
    io::{self, Write},
    path::PathBuf,
};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "seance-build")]
#[command(about = "Rust-owned release tooling for Seance")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    PrepareRelease {
        #[arg(long)]
        tag_ref: String,
        #[arg(long)]
        release_dir: PathBuf,
        #[arg(long)]
        manifest_out: PathBuf,
        #[arg(long)]
        release_notes_out: PathBuf,
    },
    EnsureDraftRelease {
        #[arg(long)]
        manifest: PathBuf,
    },
    Version {
        #[arg(long, default_value = "seance-app")]
        package: String,
        #[arg(long)]
        expect_tag: Option<String>,
    },
    ReleaseNotes {
        #[arg(long)]
        version: String,
        #[arg(long, default_value = "CHANGELOG.md")]
        changelog: PathBuf,
    },
    ArtifactPaths {
        #[arg(long, value_parser = ["macos-release"])]
        group: String,
        #[arg(long)]
        release_dir: PathBuf,
    },
    ArtifactName {
        #[arg(long, value_parser = ["macos-app-zip", "macos-dmg", "linux-appimage", "linux-zsync", "sparkle-item", "checksums"])]
        kind: String,
        #[arg(long)]
        arch: Option<String>,
    },
    LinuxTargetTriple {
        #[arg(long, value_parser = ["x86_64", "aarch64"])]
        arch: String,
    },
    LinuxUpdateInformation {
        #[arg(long, value_parser = ["x86_64", "aarch64"])]
        arch: String,
        #[arg(long)]
        repo_slug: String,
    },
    ReleaseArtifacts {
        #[arg(long)]
        include_metadata: bool,
    },
    WriteSparkleItem {
        #[arg(long)]
        version: String,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "/usr/local/bin/sign_update")]
        sign_update: PathBuf,
        #[arg(long)]
        pub_date: Option<String>,
    },
    GenerateAppcast {
        #[arg(long)]
        item: PathBuf,
        #[arg(long)]
        download_url: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        release_notes_url: Option<String>,
    },
    ValidateArtifacts {
        #[arg(long)]
        release_dir: PathBuf,
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
    },
    ValidateReleaseDir {
        #[arg(long)]
        release_dir: PathBuf,
        #[arg(long)]
        include_metadata: bool,
    },
    WriteChecksums {
        #[arg(long)]
        release_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    WriteChecksumsFromManifest {
        #[arg(long)]
        manifest: PathBuf,
    },
    WritePlatformManifest {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, value_parser = ["macos", "linux"])]
        platform: String,
        #[arg(long, value_parser = ["x86_64", "aarch64"])]
        arch: String,
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<PathBuf>,
    },
    UploadPlatformAssets {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, value_parser = ["macos", "linux"])]
        platform: String,
        #[arg(long, value_parser = ["x86_64", "aarch64"])]
        arch: String,
    },
    ValidateFromManifest {
        #[arg(long)]
        manifest: PathBuf,
    },
    GenerateAppcastFromManifest {
        #[arg(long)]
        manifest: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::PrepareRelease {
            tag_ref,
            release_dir,
            manifest_out,
            release_notes_out,
        } => manifest::prepare_release(&tag_ref, &release_dir, &manifest_out, &release_notes_out)?,
        Command::EnsureDraftRelease { manifest } => manifest::ensure_draft_release(&manifest)?,
        Command::Version {
            package,
            expect_tag,
        } => println!(
            "{}",
            workspace::verified_package_version(&package, expect_tag.as_deref())?
        ),
        Command::ReleaseNotes { version, changelog } => {
            let notes = changelog::release_notes(&changelog, &version)?;
            let mut stdout = io::stdout().lock();
            stdout.write_all(notes.as_bytes())?;
            stdout.write_all(b"\n")?;
        }
        Command::ArtifactPaths { group, release_dir } => {
            for path in artifacts::artifact_paths(&group, &release_dir)? {
                println!("{path}");
            }
        }
        Command::ArtifactName { kind, arch } => {
            println!("{}", artifacts::artifact_name(&kind, arch.as_deref())?)
        }
        Command::LinuxTargetTriple { arch } => {
            println!("{}", artifacts::linux_target_triple(&arch)?)
        }
        Command::LinuxUpdateInformation { arch, repo_slug } => {
            println!(
                "{}",
                artifacts::linux_update_information(&repo_slug, &arch)?
            )
        }
        Command::ReleaseArtifacts { include_metadata } => {
            for artifact in artifacts::release_artifacts(include_metadata) {
                println!("{artifact}");
            }
        }
        Command::WriteSparkleItem {
            version,
            artifact,
            output,
            sign_update,
            pub_date,
        } => sparkle::write_sparkle_item(
            &version,
            &artifact,
            &output,
            &sign_update,
            pub_date.as_deref(),
        )?,
        Command::GenerateAppcast {
            item,
            download_url,
            output,
            release_notes_url,
        } => sparkle::write_appcast(&item, &download_url, release_notes_url.as_deref(), &output)?,
        Command::ValidateArtifacts {
            release_dir,
            artifacts,
        } => sparkle::validate_artifacts(&release_dir, &artifacts)?,
        Command::ValidateReleaseDir {
            release_dir,
            include_metadata,
        } => artifacts::validate_release_dir(&release_dir, include_metadata)?,
        Command::WriteChecksums {
            release_dir,
            output,
        } => artifacts::write_checksums(&release_dir, &output)?,
        Command::WriteChecksumsFromManifest { manifest } => {
            manifest::write_checksums_from_manifest(&manifest)?
        }
        Command::WritePlatformManifest {
            manifest,
            platform,
            arch,
            artifacts,
        } => manifest::write_platform_manifest(&manifest, &platform, &arch, &artifacts)?,
        Command::UploadPlatformAssets {
            manifest,
            platform,
            arch,
        } => manifest::upload_platform_assets(&manifest, &platform, &arch)?,
        Command::ValidateFromManifest { manifest } => manifest::validate_from_manifest(&manifest)?,
        Command::GenerateAppcastFromManifest { manifest } => {
            manifest::generate_appcast_from_manifest(&manifest)?
        }
    }

    Ok(())
}
