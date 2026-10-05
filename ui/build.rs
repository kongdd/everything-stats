use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("TARGET").as_deref() != Ok("x86_64-pc-windows-gnu") {
        return;
    }
    // Some MinGW distributions omit ktmw32; windows-registry needs these two imports.
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let definition = output.join("ktmw32.def");
    fs::write(
        &definition,
        "LIBRARY ktmw32.dll\nEXPORTS\nCommitTransaction\nCreateTransaction\n",
    )
    .expect("cannot write ktmw32 import definition");
    let status = Command::new("dlltool")
        .args(["-m", "i386:x86-64", "-d"])
        .arg(definition)
        .arg("-l")
        .arg(output.join("libktmw32.a"))
        .status()
        .expect("install MinGW binutils (dlltool) for the Windows GNU target");
    assert!(status.success(), "cannot build ktmw32 import library");
    println!("cargo:rustc-link-search=native={}", output.display());
}
