// nami's examples (examples/packages, projects, services), built into nami
// as files `nami new` copies into your folders. nami reads none of them
// itself: what a computer or project uses is what's in your files. Whatever
// is there is built in: the public repo has no namiOS packages (namios,
// namios-*), this one does, and a missing folder is just empty.
use std::fmt::Write;
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    // namiOS's part of nami (namios/, src/nami_os.rs): built in when it's here, as the
    // examples are. The public repo hasn't it; NAMI_WITHOUT_NAMIOS=1 builds what it builds.
    // A Mac's nami never has it, nor an ARM computer's: namiOS is an x86_64 Linux system
    // (docs/macos.md, docs/arm.md).
    println!("cargo::rustc-check-cfg=cfg(namios)");
    println!("cargo:rerun-if-env-changed=NAMI_WITHOUT_NAMIOS");
    let namios = Path::new("namios/src/mod.rs");
    if namios.exists() {
        // (Only when it's here: a path that isn't would run this again at every build.)
        println!("cargo:rerun-if-changed=namios/src/mod.rs");
        let linux = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux");
        let x86_64 = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "x86_64");
        if linux && x86_64 && std::env::var_os("NAMI_WITHOUT_NAMIOS").is_none_or(|v| v.is_empty() || v == "0") {
            println!("cargo:rustc-cfg=namios");
        }
    }
    let mut files = Vec::new();
    for d in ["packages", "projects", "services"] {
        let dir = Path::new("examples").join(d);
        println!("cargo:rerun-if-changed={}", dir.display());
        walk(&dir, &mut files);
    }
    files.sort();
    let mut code = String::from("pub(crate) const EXAMPLES: &[(&str, &[u8])] = &[\n");
    for f in files {
        let rel = f.strip_prefix("examples").unwrap().display().to_string();
        writeln!(code, "    ({rel:?}, include_bytes!({:?})),", f.canonicalize().unwrap().display().to_string()).unwrap();
    }
    code.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("examples.rs"), code).unwrap();
    // This release's notes (docs/releases/<version>.md), for Updates' What's New
    // once it's installed (src/selfupdate.rs).
    let notes = Path::new("docs/releases").join(format!("{}.md", std::env::var("CARGO_PKG_VERSION").unwrap()));
    println!("cargo:rerun-if-changed=docs/releases");
    let code = match notes.canonicalize() {
        Ok(f) => format!("Some(include_str!({:?}))", f.display().to_string()),
        Err(_) => "None".to_string(),
    };
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("notes.rs"), code).unwrap();
}
