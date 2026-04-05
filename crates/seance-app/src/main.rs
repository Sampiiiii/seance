use std::fs;

use anyhow::Context;
use seance_vault::VaultStore;

fn main() -> anyhow::Result<()> {
    let data_root = dirs::data_local_dir().unwrap_or(std::env::current_dir()?);
    let app_root = data_root.join("seance");
    fs::create_dir_all(&app_root).context("failed to create app data directory")?;

    let vault = VaultStore::open(app_root.join("vault.sqlite"))
        .context("failed to open the encrypted vault")?;

    seance_ui::run(vault)
}
