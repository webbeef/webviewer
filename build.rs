/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::env;

use gl_generator::{Api, Fallbacks, Profile, Registry};
use vergen::EmitBuilder;

// We can make this configurable in the future if different platforms start to have
// different needs.
fn generate_egl_bindings(out_dir: &Path) {
    let mut file = File::create(out_dir.join("egl_bindings.rs")).unwrap();
    Registry::new(Api::Egl, (1, 5), Profile::Core, Fallbacks::All, [])
        .write_bindings(gl_generator::StaticStructGenerator, &mut file)
        .unwrap();
    println!("cargo:rustc-link-lib=EGL");
}

fn find_python() -> String {
    env::var("PYTHON3").ok().unwrap_or_else(|| {
        let candidates = if cfg!(windows) {
            ["python.exe", "python"]
        } else {
            ["python3", "python"]
        };
        for &name in &candidates {
            if Command::new(name)
                .arg("--version")
                .output()
                .ok()
                .map_or(false, |out| out.status.success())
            {
                return name.to_owned();
            }
        }
        panic!(
            "Can't find python (tried {})! Try fixing PATH or setting the PYTHON3 env var",
            candidates.join(", ")
        )
    })
}

// Generate the WebIDL bindings with Servo's codegen.
fn generate_webidl_bindings() {
    let servo_path = if let Some(servo_env_path) = env::var_os("SERVO_PATH") {
        servo_env_path.into_string().unwrap()
    } else {
        panic!("Set SERVO_PATH to the root of your servo repository to build local webidl bindings.");
    };

    // TODO: Don't hardcode..
    let cwd = env::current_dir().unwrap();
    let style_out_dir = PathBuf::from(format!("{}/target/release/build/style-712769e00544c534/out/css-properties.json", cwd.display()));

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let status = Command::new(find_python())
        .arg(format!("{}/components/script/dom/bindings/codegen/run.py", servo_path))
        .arg(style_out_dir)
        .arg(cwd.join("webidls"))
        .arg(&out_dir)
        .status()
        .unwrap();
    if !status.success() {
        std::process::exit(1)
    }
}


fn main() -> Result<(), Box<dyn Error>> {
    generate_webidl_bindings();

    println!("cargo::rustc-check-cfg=cfg(servo_production)");
    println!("cargo::rustc-check-cfg=cfg(servo_do_not_use_in_production)");
    // Cargo does not expose the profile name to crates or their build scripts,
    // but we can extract it from OUT_DIR and set a custom cfg() ourselves.
    let out = env::var("OUT_DIR")?;
    let out = Path::new(&out);
    let krate = out.parent().unwrap();
    let build = krate.parent().unwrap();
    let profile = build
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy();
    if profile == "production" || profile.starts_with("production-") {
        println!("cargo:rustc-cfg=servo_production");
    } else {
        println!("cargo:rustc-cfg=servo_do_not_use_in_production");
    }

    // Note: We can't use `#[cfg(windows)]`, since that would check the host platform
    // and not the target platform
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();

    if target_os == "windows" {
        #[cfg(windows)]
        {
            let mut res = winres::WindowsResource::new();
            res.set_icon("../../resources/servo.ico");
            res.set_manifest_file("platform/windows/servo.exe.manifest");
            res.compile().unwrap();
        }
        #[cfg(not(windows))]
        panic!("Cross-compiling to windows is currently not supported");
    } else if target_os == "macos" {
        cc::Build::new()
            .file("platform/macos/count_threads.c")
            .compile("count_threads");
    } else if target_os == "android" {
        generate_egl_bindings(out);

        // FIXME: We need this workaround since jemalloc-sys still links
        // to libgcc instead of libunwind, but Android NDK 23c and above
        // don't have libgcc. We can't disable jemalloc for Android as
        // in 64-bit aarch builds, the system allocator uses tagged
        // pointers by default which causes the assertions in SM & mozjs
        // to fail. See https://github.com/servo/servo/issues/32175.
        let mut libgcc = File::create(out.join("libgcc.a")).unwrap();
        libgcc.write_all(b"INPUT(-lunwind)").unwrap();
        println!("cargo:rustc-link-search=native={}", out.display());
    } else if target_env == "ohos" {
        generate_egl_bindings(out);
    }

    if let Err(error) = EmitBuilder::builder()
        .fail_on_error()
        .git_sha(true /* short */)
        .emit()
    {
        println!(
            "cargo:warning=Could not generate git version information: {:?}",
            error
        );
        println!("cargo:rustc-env=VERGEN_GIT_SHA=nogit");
    }

    // On MacOS, all dylib dependencies are shipped along with the binary
    // in the "/lib" directory. Setting the rpath here, allows the dynamic
    // linker to locate them. See `man dyld` for more info.
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/lib/");
    }
    Ok(())
}
