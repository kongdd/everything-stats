fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let source = "vendor/everything3/src/Everything3.c";
    // SDK3 targets MSVC; normalize integer suffixes and one callback declaration for GCC.
    let code = std::fs::read_to_string(source)
        .unwrap()
        .replace("UI64", "ULL")
        .replace("ui32", "U")
        .replace("#include \"../include/Everything3.h\"", "#include \"Everything3.h\"")
        .replace(
            "void *callback_proc);",
            "EVERYTHING3_BOOL (EVERYTHING3_API *callback_proc)(void *,const EVERYTHING3_JOURNAL_CHANGEUTF8 *));",
        );
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(output.join("Everything3.c"), code).unwrap();
    let header = std::fs::read_to_string("vendor/everything3/include/Everything3.h")
        .unwrap()
        .replace("UI64", "ULL");
    std::fs::write(output.join("Everything3.h"), header).unwrap();
    cc::Build::new()
        .file(output.join("Everything3.c"))
        .include(&output)
        .define("EVERYTHING3_LIB", None)
        .flag_if_supported("-Wno-error=incompatible-pointer-types")
        .warnings(false)
        .compile("everything3");
    println!("cargo:rustc-link-lib=ole32");
    println!("cargo:rerun-if-changed={source}");
    println!("cargo:rerun-if-changed=vendor/everything3/include/Everything3.h");
}
