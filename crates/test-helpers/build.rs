use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());

    let mut wasms = Vec::new();

    if cfg!(feature = "wasm-rust") {
        let mut cmd = Command::new("cargo");
        cmd.arg("build")
            .current_dir("../test-rust-wasm")
            .arg("--target=wasm32-wasi")
            .env("CARGO_TARGET_DIR", &out_dir)
            .env("CARGO_PROFILE_DEV_DEBUG", "1")
            .env("RUSTFLAGS", "-Clink-args=--export-table")
            .env_remove("CARGO_ENCODED_RUSTFLAGS");
        let status = cmd.status().unwrap();
        assert!(status.success());
        for file in out_dir.join("wasm32-wasi/debug").read_dir().unwrap() {
            let file = file.unwrap().path();
            if file.extension().and_then(|s| s.to_str()) != Some("wasm") {
                continue;
            }
            wasms.push((
                "rust",
                file.file_stem().unwrap().to_str().unwrap().to_string(),
                file.to_str().unwrap().to_string(),
            ));

            let dep_file = file.with_extension("d");
            let deps = fs::read_to_string(&dep_file).expect("failed to read dep file");
            for dep in deps
                .splitn(2, ":")
                .skip(1)
                .next()
                .unwrap()
                .split_whitespace()
            {
                println!("cargo:rerun-if-changed={}", dep);
            }
        }
        println!("cargo:rerun-if-changed=../test-rust-wasm/Cargo.toml");
    }

    let src = format!("const WASMS: &[(&str, &str, &str)] = &{:?};", wasms);
    std::fs::write(out_dir.join("wasms.rs"), src).unwrap();
}
