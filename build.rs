use std::env;
use std::path::Path;
use std::process::Command;

fn add_link_search(path: &str) {
    if Path::new(path).exists() {
        println!("cargo:rustc-link-search=native={}", path);
    }
}

fn add_brew_prefix(pkg: &str) {
    let output = Command::new("brew").args(["--prefix", pkg]).output();

    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(prefix) = String::from_utf8(output.stdout) {
                let prefix = prefix.trim();
                if !prefix.is_empty() {
                    add_link_search(&format!("{}/lib", prefix));
                }
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=HOMEBREW_PREFIX");
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        add_link_search(&format!("{}/native", manifest_dir));
    }
    if let Ok(prefix) = env::var("HOMEBREW_PREFIX") {
        add_link_search(&format!("{}/lib", prefix));
    }

    add_link_search("/opt/homebrew/lib");
    add_link_search("/usr/local/lib");
    add_link_search("/opt/homebrew/opt/leptonica/lib");
    add_link_search("/opt/homebrew/opt/tesseract/lib");
    add_link_search("/usr/local/opt/leptonica/lib");
    add_link_search("/usr/local/opt/tesseract/lib");

    add_brew_prefix("leptonica");
    add_brew_prefix("tesseract");
}
