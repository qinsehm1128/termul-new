fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "native")]
    native::build();
}

#[cfg(feature = "native")]
mod native {
    use sha2::{Digest, Sha256};
    use std::{
        env, fs,
        io::Read,
        path::{Component, Path, PathBuf},
        process::Command,
    };

    const SHA256: &str = "af8cd2fbd25c17990b25c8bf8352263c35c5ec446134220de867236497311edd";
    const VERSION: &str = "1.3.2-HEAD-+44f2a44df";
    const VENDORED_PACKAGES_DIR: &str = "../../vendor/ghostty-vt/zig-packages";
    /// Every Zig package the VT build fetches, mirrored so a flaky upstream
    /// host (codeberg 504s took down a release) cannot break the build.
    /// Names are Zig content hashes; Zig re-verifies them on extraction.
    const VENDORED_PACKAGES: [(&str, &str); 8] = [
        (
            "aro-0.0.0-JSD1Qk6lNgDdcDV4Vh7Sfy-34m2TluIVOdPzMmj_0BjX",
            "563ebdb54ec3a18d475e47cc362b61ef1f5d6b350134565c7cfe304158901d14",
        ),
        (
            "N-V-__8AAB0eQwD-0MdOEBmz7intriBReIsIDNlukNVoNu6o",
            "afedfc21da5486fe7124a8de8cf496f912dafb8de743c9dd921981d0847481e9",
        ),
        (
            "N-V-__8AADYiAAB_80AWnH1AxXC0tql9thT-R-DYO1gBqTLc",
            "47f217a26e815226bc045409727d3d78af7aad9bd7a3f53cfa48fd085575e70b",
        ),
        (
            "N-V-__8AAEFmBABuDGOKxAI6VMg41b9euMZ-z7HS9EcUdaor",
            "6367e3058e4ea1a6284d1d47677ff9f1905ce2fe0f5aad0a9611db602ce617b5",
        ),
        (
            "N-V-__8AAGmZhABbsPJLfbqrh6JTHsXhY6qCaLAQyx25e0XE",
            "cf0f68a4275e59282383f46289da017166ea4cbbced04ad2af1b79f3eede3cc2",
        ),
        (
            "N-V-__8AAP5JWgCGP_AD0teWpa4krRvE9VPZzvviGdbmN4jI",
            "17877d346f7988651d6c62e45b2e3b05c86809a6442c5e34612374b2a9719a1c",
        ),
        (
            "translate_c-0.0.0-Q_BUWhVNBwDOEcIqub4VFPJPB6D9dgwzUMHTX5KWr8Xr",
            "f59dab4e74b399e9d00c44f5d667c5c0d212ddfaf72537b593d76514dd59ec8c",
        ),
        (
            "uucode-0.2.0-ZZjBPlK5VADj7fdoq7G8LIHzD5o6FSkcBXXrRWr4jnrA",
            "3c571e2e2c1dd6d67d59e7a29322c718a90465ccb0df362e14feafcdde555ed0",
        ),
    ];

    pub fn build() {
        for key in [
            "SE_GHOSTTY_VT_ZIG",
            "SE_GHOSTTY_VT_PREBUILT_DIR",
            "ZIG_LOCAL_CACHE_DIR",
            "ZIG_GLOBAL_CACHE_DIR",
        ] {
            println!("cargo:rerun-if-env-changed={key}");
        }
        println!("cargo:rerun-if-changed=native/shim.c");
        let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let archive = manifest.join("../../vendor/ghostty-vt/ghostty-vt-44f2a44df-source.tar.gz");
        println!("cargo:rerun-if-changed={}", archive.display());
        let bytes = fs::read(&archive).expect("read pinned Ghostty VT archive");
        assert_eq!(digest(&bytes), SHA256, "Ghostty VT source SHA256 mismatch");
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let source = out.join("source");
        if source.exists() {
            fs::remove_dir_all(&source).expect("remove old extracted source");
        }
        fs::create_dir_all(&source).unwrap();
        extract(&bytes, &source);
        let source = source.join("ghostty-vt");
        let target = env::var("TARGET").unwrap();
        let lib_name = if target.contains("windows-msvc") {
            // The un-suffixed .lib is the DLL import library. Only this
            // combined static archive keeps self-exec hosts DLL-independent.
            "ghostty-vt-static.lib"
        } else {
            "libghostty-vt.a"
        };
        let prefix = out.join("native");
        if let Some(prebuilt) = env::var_os("SE_GHOSTTY_VT_PREBUILT_DIR") {
            // Explicit local trust opt-in. Both source identity and binary digest
            // must match its manifest. The C shim also checks runtime version.
            let prebuilt = PathBuf::from(prebuilt);
            let metadata_path = prebuilt.join("se-ghostty-vt.manifest");
            println!("cargo:rerun-if-changed={}", metadata_path.display());
            let metadata = fs::read_to_string(&metadata_path)
                .expect("prebuilt requires se-ghostty-vt.manifest; see README");
            let get = |key: &str| {
                metadata
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{key}=")))
                    .unwrap_or("")
            };
            assert_eq!(get("source_sha256"), SHA256, "prebuilt source mismatch");
            assert_eq!(get("version"), VERSION, "prebuilt version mismatch");
            assert_eq!(get("target"), target, "prebuilt target mismatch");
            let library_path = prebuilt.join("lib").join(lib_name);
            println!("cargo:rerun-if-changed={}", library_path.display());
            let library = fs::read(library_path).expect("read prebuilt static library");
            assert_eq!(
                get("library_sha256"),
                digest(&library),
                "prebuilt binary SHA256 mismatch"
            );
            fs::create_dir_all(prefix.join("lib")).unwrap();
            fs::write(prefix.join("lib").join(lib_name), library).unwrap();
        } else {
            let zig = env::var_os("SE_GHOSTTY_VT_ZIG").unwrap_or_else(|| "zig".into());
            let version = Command::new(&zig)
                .arg("version")
                .output()
                .expect("native feature requires Zig 0.16.0 (SE_GHOSTTY_VT_ZIG)");
            assert!(
                version.status.success()
                    && String::from_utf8_lossy(&version.stdout).trim() == "0.16.0",
                "native feature requires exactly Zig 0.16.0"
            );
            let zig_target = zig_target(&target);
            let global_cache = env::var_os("ZIG_GLOBAL_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| out.join("zig-global-cache"));
            seed_zig_packages(&manifest.join(VENDORED_PACKAGES_DIR), &global_cache);
            let output = Command::new(zig)
                .current_dir(&source)
                .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
                .args([
                    "build",
                    "-Demit-lib-vt",
                    "-Demit-xcframework=false",
                    "-Doptimize=ReleaseFast",
                ])
                .arg(format!("-Dtarget={zig_target}"))
                .arg(format!("-Dversion-string={VERSION}"))
                .arg("--prefix")
                .arg(&prefix)
                .output()
                .expect("execute Zig VT build");
            if !output.status.success() {
                panic!(
                    "Ghostty VT Zig build failed (including dependency fetch diagnostics):\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        assert!(
            prefix.join("lib").join(lib_name).is_file(),
            "Zig did not produce the expected static VT library"
        );
        cc::Build::new()
            .file("native/shim.c")
            .include(source.join("include"))
            .flag_if_supported("-std=c11")
            .warnings(true)
            .compile("se_ghostty_vt_shim");
        println!(
            "cargo:rustc-link-search=native={}",
            prefix.join("lib").display()
        );
        if target.contains("windows-msvc") {
            println!("cargo:rustc-link-lib=static=ghostty-vt-static");
        } else {
            println!("cargo:rustc-link-lib=static=ghostty-vt");
        }
        if target.contains("windows") {
            println!("cargo:rustc-link-lib=ntdll");
            println!("cargo:rustc-link-lib=kernel32");
        }
    }

    /// Places the vendored package archives where `zig build` looks before it
    /// would download: `<global cache>/p/<hash>.tar.gz`.
    fn seed_zig_packages(vendor: &Path, global_cache: &Path) {
        let packages = global_cache.join("p");
        fs::create_dir_all(&packages).expect("create Zig package cache");
        for (name, sha256) in VENDORED_PACKAGES {
            let file = format!("{name}.tar.gz");
            let source = vendor.join(&file);
            println!("cargo:rerun-if-changed={}", source.display());
            let bytes = fs::read(&source)
                .unwrap_or_else(|e| panic!("read vendored Zig package {file}: {e}"));
            assert_eq!(
                digest(&bytes),
                sha256,
                "vendored Zig package {file} SHA256 mismatch"
            );
            let target = packages.join(&file);
            if fs::read(&target)
                .map(|existing| existing != bytes)
                .unwrap_or(true)
            {
                fs::write(&target, &bytes).expect("seed Zig package cache");
            }
        }
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn zig_target(target: &str) -> String {
        let arch = target.split('-').next().unwrap();
        match target {
            t if t.ends_with("apple-darwin") => format!("{arch}-macos"),
            t if t.ends_with("unknown-linux-gnu") => format!("{arch}-linux-gnu"),
            t if t.ends_with("unknown-linux-musl") => format!("{arch}-linux-musl"),
            t if t.ends_with("pc-windows-msvc") => format!("{arch}-windows-msvc"),
            _ => panic!("unsupported Ghostty VT native target: {target}"),
        }
    }

    fn extract(bytes: &[u8], destination: &Path) {
        let decoder = flate2::read::GzDecoder::new(bytes);
        // Hard cap even if the pinned input is replaced in a future update.
        let limited = decoder.take(128 * 1024 * 1024);
        let mut archive = tar::Archive::new(limited);
        let mut count = 0;
        let mut total = 0u64;
        for entry in archive.entries().expect("read source tar") {
            let mut entry = entry.expect("read source tar entry");
            count += 1;
            assert!(count <= 2000, "too many source entries");
            assert!(
                entry.header().entry_type().is_file(),
                "source archive permits only regular files"
            );
            let path = entry.path().expect("source entry path").into_owned();
            assert!(
                path.components().all(|c| matches!(c, Component::Normal(_))),
                "unsafe source archive path"
            );
            assert!(path.starts_with("ghostty-vt"), "unexpected source root");
            let size = entry.size();
            total = total.checked_add(size).expect("source size overflow");
            assert!(
                size <= 16 * 1024 * 1024 && total <= 128 * 1024 * 1024,
                "source archive exceeds extraction limits"
            );
            assert!(
                entry.unpack_in(destination).expect("extract pinned source"),
                "unsafe source entry"
            );
        }
        assert_eq!(count, 1339, "pinned source file count mismatch");
    }
}
