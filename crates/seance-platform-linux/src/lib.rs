use anyhow::Result;
use seance_platform::{PlatformApp, PlatformRuntime};

pub struct LinuxPlatformRuntime;

impl PlatformRuntime for LinuxPlatformRuntime {
    fn run(self, mut app: Box<dyn PlatformApp>) -> Result<()> {
        app.on_launch()
    }
}
