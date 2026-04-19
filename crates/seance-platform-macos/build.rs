use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=src/sparkle_bridge.m");
        println!("cargo:rerun-if-changed=src/dock_icon_bridge.m");
        println!("cargo:rerun-if-changed=src/promotion_bridge.m");

        let objects = cc::Build::new()
            .files([
                "src/sparkle_bridge.m",
                "src/dock_icon_bridge.m",
                "src/promotion_bridge.m",
            ])
            .flag("-fblocks")
            .flag("-std=gnu11")
            .flag("-Wall")
            .flag("-fobjc-arc")
            .compile_intermediates();

        assert!(
            !objects.is_empty(),
            "expected at least one object file from macOS bridge sources"
        );

        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
        let archive_path = out_dir.join("libseance_sparkle_bridge.a");

        let archive_path = archive_path
            .to_str()
            .expect("archive path must be valid UTF-8")
            .to_owned();
        let mut command = Command::new("xcrun");
        command
            .arg("libtool")
            .arg("-static")
            .arg("-o")
            .arg(&archive_path);
        for object in &objects {
            command.arg(object.to_str().expect("object path must be valid UTF-8"));
        }

        let output = command
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .expect("failed to invoke xcrun libtool");

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "xcrun libtool failed with status {}.\nstdout:\n{}\nstderr:\n{}",
                output.status, stdout, stderr
            );
        }

        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=seance_sparkle_bridge");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=ImageIO");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=QuartzCore");
    }
}
