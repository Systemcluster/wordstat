use std::{env::var, process::Command};
use which::which;

fn get_git_hash() -> Option<String> {
    which("git").ok().and_then(|git| {
        Command::new(git)
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8(output.stdout)
                    .map(|output| output.trim().into())
                    .ok()
            })
    })
}

fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/book.ico");
        res.set_manifest_file("resources/manifest.xml");
        res.compile().unwrap();
    }

    if let Some(hash) = get_git_hash() {
        println!("cargo:rustc-env=GIT_HASH={}", hash);
        println!(
            "cargo:rustc-env=CARGO_PKG_VERSION={} ({})",
            var("CARGO_PKG_VERSION").unwrap(),
            hash
        );
    }
    println!("cargo:rerun-if-changed=.git/HEAD");
}
