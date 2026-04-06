use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=src/sparkle_bridge.m");

        let objects = cc::Build::new()
            .file("src/sparkle_bridge.m")
            .flag("-fobjc-arc")
            .compile_intermediates();

        assert_eq!(
            objects.len(),
            1,
            "expected exactly one object file from sparkle_bridge.m, got {}",
            objects.len()
        );

        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
        let archive_path = out_dir.join("libseance_sparkle_bridge.a");

        let output = Command::new("xcrun")
            .args([
                "libtool",
                "-static",
                "-o",
                archive_path
                    .to_str()
                    .expect("archive path must be valid UTF-8"),
                objects[0]
                    .to_str()
                    .expect("object path must be valid UTF-8"),
            ])
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
    }
}
