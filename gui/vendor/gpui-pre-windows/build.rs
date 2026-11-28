#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

// nami: for the Windows build (target), wherever it's built: cfg! here is
// the machine building, so the target is asked of cargo.
//
// The shaders come compiled, in shaders/ (DXBC, as fxc /T vs_4_1|ps_4_1 /O3
// makes them), with the SHA-256 of the HLSL they were compiled from
// (shaders/source.sha256), so building needs no Windows: they're used while
// that HLSL is unchanged. Only when it changes does fxc (the Windows SDK's,
// or GPUI_FXC_PATH) compile them, which needs Windows; GPUI_SHADERS_SAVE=1
// also writes them back into shaders/, to be committed. On Windows, in Git
// Bash, from gui/:
//   GPUI_SHADERS_SAVE=1 cargo build --release --target x86_64-pc-windows-msvc
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GPUI_FXC_PATH");
    println!("cargo:rerun-if-env-changed=GPUI_SHADERS_SAVE");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_none() {
        compile_shaders();
    }
}

mod shader_compilation {
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::Write,
        path::{Path, PathBuf},
        process::{self, Command},
    };

    // Each module (a vertex and a fragment shader), and the file it's in.
    const MODULES: [(&str, &str); 9] = [
        ("quad", "src/shaders.hlsl"),
        ("shadow", "src/shaders.hlsl"),
        ("path_rasterization", "src/shaders.hlsl"),
        ("path_sprite", "src/shaders.hlsl"),
        ("underline", "src/shaders.hlsl"),
        ("monochrome_sprite", "src/shaders.hlsl"),
        ("subpixel_sprite", "src/shaders.hlsl"),
        ("polychrome_sprite", "src/shaders.hlsl"),
        ("emoji_rasterization", "src/color_text_raster.hlsl"),
    ];
    // File suffix, and the constant's (QUAD_VERTEX_BYTES).
    const STAGES: [(&str, &str); 2] = [("vs", "VERTEX"), ("ps", "FRAGMENT")];
    const SOURCES: [&str; 2] = ["src/shaders.hlsl", "src/color_text_raster.hlsl"];

    /// The sources' SHA-256, as sha256sum writes it (`sha256sum -c` checks it, from this crate's folder).
    fn sources_sha256(dir: &Path) -> String {
        SOURCES
            .iter()
            .map(|source| {
                let bytes = fs::read(dir.join(source)).expect("Failed to read shader source");
                let hash: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
                format!("{hash}  {source}\n")
            })
            .collect()
    }

    /// Whether shaders/ has every shader, compiled from the HLSL as it is now.
    fn compiled_match(dir: &Path) -> bool {
        let compiled = dir.join("shaders");
        fs::read_to_string(compiled.join("source.sha256")).is_ok_and(|recorded| recorded == sources_sha256(dir))
            && MODULES.iter().all(|(module, _)| {
                STAGES.iter().all(|(stage, _)| compiled.join(format!("{module}_{stage}.dxbc")).is_file())
            })
    }

    pub fn compile_shaders() {
        let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let rust_binding_path = format!("{}/shaders_bytes.rs", out_dir);
        for source in SOURCES {
            println!("cargo:rerun-if-changed={}", dir.join(source).display());
        }
        println!("cargo:rerun-if-changed={}", dir.join("shaders").display());

        let save = std::env::var_os("GPUI_SHADERS_SAVE").is_some_and(|v| !v.is_empty());
        if !save && compiled_match(&dir) {
            let mut binding = String::new();
            for (module, _) in MODULES {
                for (stage, name) in STAGES {
                    let blob = dir.join("shaders").join(format!("{module}_{stage}.dxbc"));
                    binding += &format!(
                        "const {}_{name}_BYTES: &[u8] = include_bytes!({:?});\n",
                        module.to_uppercase(),
                        blob.to_str().unwrap()
                    );
                }
            }
            fs::write(&rust_binding_path, binding).expect("Failed to write Rust binding file");
            return;
        }

        let fxc_path = find_fxc_compiler();
        if Path::new(&rust_binding_path).exists() {
            fs::remove_file(&rust_binding_path)
                .expect("Failed to remove existing Rust binding file");
        }
        for (module, source) in MODULES {
            compile_shader_for_module(
                module,
                &out_dir,
                &fxc_path,
                dir.join(source).to_str().unwrap(),
                &rust_binding_path,
            );
        }

        if save {
            let compiled = dir.join("shaders");
            fs::create_dir_all(&compiled).expect("Failed to create shaders/");
            for (module, _) in MODULES {
                for (stage, _) in STAGES {
                    let header = fs::read_to_string(format!("{out_dir}/{module}_{stage}.h"))
                        .expect("Failed to read header file");
                    fs::write(compiled.join(format!("{module}_{stage}.dxbc")), header_bytes(&header))
                        .expect("Failed to write compiled shader");
                }
            }
            fs::write(compiled.join("source.sha256"), sources_sha256(&dir))
                .expect("Failed to write shaders/source.sha256");
            println!("cargo::warning=gpui's shaders compiled into {}: commit them", compiled.display());
        }
    }

    /// The bytes in fxc's /Fh header (`const BYTE name[] = { 0x44, ... };`).
    fn header_bytes(header: &str) -> Vec<u8> {
        let body = &header[header.find("const BYTE").unwrap()..];
        let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
        body.split(',')
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| u8::from_str_radix(b.trim_start_matches("0x"), 16).expect("Bad byte in fxc's header"))
            .collect()
    }

    /// Locate `binary` in the newest installed Windows SDK.
    #[cfg(not(target_os = "windows"))]
    pub fn find_latest_windows_sdk_binary(_: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        Ok(None)
    }

    #[cfg(target_os = "windows")]
    pub fn find_latest_windows_sdk_binary(
        binary: &str,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        let key = windows_registry::LOCAL_MACHINE
            .open("SOFTWARE\\WOW6432Node\\Microsoft\\Microsoft SDKs\\Windows\\v10.0")?;

        let install_folder: String = key.get_string("InstallationFolder")?; // "C:\Program Files (x86)\Windows Kits\10\"
        let install_folder_bin = Path::new(&install_folder).join("bin");

        let mut versions: Vec<_> = std::fs::read_dir(&install_folder_bin)?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();

        versions.sort_by_key(|s| {
            s.split('.')
                .filter_map(|p| p.parse().ok())
                .collect::<Vec<u32>>()
        });

        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => Err(format!(
                "Unsupported architecture: {}",
                std::env::consts::ARCH
            ))?,
        };

        if let Some(highest_version) = versions.last() {
            return Ok(Some(
                install_folder_bin
                    .join(highest_version)
                    .join(arch)
                    .join(binary),
            ));
        }

        Ok(None)
    }

    /// You can set the `GPUI_FXC_PATH` environment variable to specify the path to the fxc.exe compiler.
    fn find_fxc_compiler() -> String {
        // Check environment variable
        if let Ok(path) = std::env::var("GPUI_FXC_PATH")
            && Path::new(&path).exists()
        {
            return path;
        }

        // Try to find in PATH
        // NOTE: This has to be `where.exe` on Windows, not `where`, it must be ended with `.exe`
        if let Ok(output) = std::process::Command::new("where.exe")
            .arg("fxc.exe")
            .output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout);
            return path.trim().to_string();
        }

        if let Ok(Some(path)) = find_latest_windows_sdk_binary("fxc.exe") {
            return path.to_string_lossy().into_owned();
        }

        panic!(
            "gpui's shaders (src/*.hlsl) aren't the ones compiled into shaders/ (see \
             shaders/source.sha256), and there's no fxc.exe to compile them (it's Windows'): \
             compile them once on Windows with GPUI_SHADERS_SAVE=1 (see build.rs) and commit shaders/"
        );
    }

    fn compile_shader_for_module(
        module: &str,
        out_dir: &str,
        fxc_path: &str,
        shader_path: &str,
        rust_binding_path: &str,
    ) {
        // Compile vertex shader
        let output_file = format!("{}/{}_vs.h", out_dir, module);
        let const_name = format!("{}_VERTEX_BYTES", module.to_uppercase());
        compile_shader_impl(
            fxc_path,
            &format!("{module}_vertex"),
            &output_file,
            &const_name,
            shader_path,
            "vs_4_1",
        );
        generate_rust_binding(&const_name, &output_file, rust_binding_path);

        // Compile fragment shader
        let output_file = format!("{}/{}_ps.h", out_dir, module);
        let const_name = format!("{}_FRAGMENT_BYTES", module.to_uppercase());
        compile_shader_impl(
            fxc_path,
            &format!("{module}_fragment"),
            &output_file,
            &const_name,
            shader_path,
            "ps_4_1",
        );
        generate_rust_binding(&const_name, &output_file, rust_binding_path);
    }

    fn compile_shader_impl(
        fxc_path: &str,
        entry_point: &str,
        output_path: &str,
        var_name: &str,
        shader_path: &str,
        target: &str,
    ) {
        let output = Command::new(fxc_path)
            .args([
                "/T",
                target,
                "/E",
                entry_point,
                "/Fh",
                output_path,
                "/Vn",
                var_name,
                "/O3",
                shader_path,
            ])
            .output();

        match output {
            Ok(result) => {
                if result.status.success() {
                    return;
                }
                println!(
                    "cargo::error=Shader compilation failed for {}:\n{}",
                    entry_point,
                    String::from_utf8_lossy(&result.stderr)
                );
                process::exit(1);
            }
            Err(e) => {
                println!("cargo::error=Failed to run fxc for {}: {}", entry_point, e);
                process::exit(1);
            }
        }
    }

    fn generate_rust_binding(const_name: &str, head_file: &str, output_path: &str) {
        let header_content = fs::read_to_string(head_file).expect("Failed to read header file");
        let const_definition = {
            let global_var_start = header_content.find("const BYTE").unwrap();
            let global_var = &header_content[global_var_start..];
            let equal = global_var.find('=').unwrap();
            global_var[equal + 1..].trim()
        };
        let rust_binding = format!(
            "const {}: &[u8] = &{}\n",
            const_name,
            const_definition.replace('{', "[").replace('}', "]")
        );
        let mut options = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output_path)
            .expect("Failed to open Rust binding file");
        options
            .write_all(rust_binding.as_bytes())
            .expect("Failed to write Rust binding file");
    }
}

use shader_compilation::compile_shaders;
