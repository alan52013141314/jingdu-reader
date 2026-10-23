use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return; }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("assets/icon.ico");
    let rc_file = out.join("icon.rc");
    fs::write(&rc_file, format!("1 ICON \"{}\"\n", icon.to_string_lossy().replace('\\', "/"))).unwrap();
    let res = out.join("icon.res");
    let mut candidates = vec![PathBuf::from("rc.exe")];
    let kits = PathBuf::from(env::var_os("ProgramFiles(x86)").unwrap_or_else(|| "C:/Program Files (x86)".into())).join("Windows Kits/10/bin");
    if let Ok(entries) = fs::read_dir(kits) {
        let mut versions: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        versions.sort(); versions.reverse();
        candidates.extend(versions.into_iter().map(|p| p.join("x64/rc.exe")));
    }
    let success = candidates.into_iter().any(|rc| Command::new(rc).args(["/nologo", "/fo"]).arg(&res).arg(&rc_file).status().is_ok_and(|s| s.success()));
    assert!(success, "Windows SDK rc.exe is required to embed the application icon");
    println!("cargo:rustc-link-arg={}", res.display());
}
