use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || !env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "gnu")
    {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lib = out.join("libshlwapi.a");
    if !lib.exists() {
        let def = out.join("shlwapi.def");
        let mut text = String::from("LIBRARY shlwapi.dll\nEXPORTS\n");
        for name in exports(r"C:\Windows\System32\shlwapi.dll") {
            text.push_str(&name);
            text.push('\n');
        }
        fs::write(&def, text).unwrap();
        let status = Command::new("dlltool")
            .args(["-m", "i386:x86-64", "-d"])
            .arg(&def)
            .arg("-l")
            .arg(&lib)
            .status()
            .expect("install MinGW dlltool");
        assert!(status.success(), "cannot build shlwapi import library");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
}

fn exports(path: &str) -> Vec<String> {
    let data = fs::read(path).expect("shlwapi.dll");
    let pe = u32::from_le_bytes(data[0x3C..0x40].try_into().unwrap()) as usize;
    let coff = pe + 4;
    let nsections = u16::from_le_bytes(data[coff + 2..coff + 4].try_into().unwrap()) as usize;
    let opt_size = u16::from_le_bytes(data[coff + 16..coff + 18].try_into().unwrap()) as usize;
    let opt = coff + 20;
    let magic = u16::from_le_bytes(data[opt..opt + 2].try_into().unwrap());
    let dir = opt + if magic == 0x20B { 112 } else { 96 };
    let sections = opt + opt_size;
    let rva_off = |rva: u32| -> usize {
        for index in 0..nsections {
            let section = sections + index * 40;
            let va = u32::from_le_bytes(data[section + 12..section + 16].try_into().unwrap());
            let raw = u32::from_le_bytes(data[section + 20..section + 24].try_into().unwrap());
            let span = u32::from_le_bytes(data[section + 8..section + 12].try_into().unwrap()).max(
                u32::from_le_bytes(data[section + 16..section + 20].try_into().unwrap()),
            );
            if rva >= va && rva < va + span {
                return (raw + rva - va) as usize;
            }
        }
        panic!("export RVA {rva:#x} is outside shlwapi.dll");
    };
    let exp = rva_off(u32::from_le_bytes(data[dir..dir + 4].try_into().unwrap()));
    let count = u32::from_le_bytes(data[exp + 24..exp + 28].try_into().unwrap()) as usize;
    let names = rva_off(u32::from_le_bytes(
        data[exp + 32..exp + 36].try_into().unwrap(),
    ));
    (0..count)
        .map(|index| {
            let rva = u32::from_le_bytes(
                data[names + index * 4..names + index * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            let off = rva_off(rva);
            let end = data[off..].iter().position(|byte| *byte == 0).unwrap();
            String::from_utf8_lossy(&data[off..off + end]).into_owned()
        })
        .collect()
}
