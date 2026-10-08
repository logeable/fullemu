use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const USER_IMAGE_BASE: usize = 0x8040_0000;
const USER_IMAGE_SLOT_SIZE: usize = 64 * 1024;
const MAX_USER_PROGRAMS: usize = 8;
const LAYOUT_FILE_ENV: &str = "FULLEMU_USER_LAYOUT_FILE";
const BOOT_MODE_ENV: &str = "FULLEMU_BOOT_MODE";
const BATCH_EXCLUDED_BINS_ENV: &str = "FULLEMU_BATCH_EXCLUDED_BINS";

fn main() {
    println!("cargo:rerun-if-changed=src/bin");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-env-changed={LAYOUT_FILE_ENV}");
    println!("cargo:rerun-if-env-changed={BOOT_MODE_ENV}");
    println!("cargo:rerun-if-env-changed={BATCH_EXCLUDED_BINS_ENV}");

    let boot_mode = env::var(BOOT_MODE_ENV).unwrap_or_else(|_| "shell".to_owned());
    if boot_mode != "shell" && boot_mode != "batch" {
        panic!("{BOOT_MODE_ENV} 只支持 shell 或 batch，当前值为：{boot_mode}");
    }
    let excluded_bins = if boot_mode == "batch" {
        env::var(BATCH_EXCLUDED_BINS_ENV)
            .unwrap_or_else(|_| "fullemu_user_shell".to_owned())
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let package_directory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo 应提供清单目录"));
    let mut binary_sources = binary_sources(&package_directory.join("src/bin"));
    binary_sources.retain(|source_path| {
        let program_name = source_path.file_stem().and_then(|name| name.to_str());
        !program_name.is_some_and(|name| excluded_bins.iter().any(|excluded| excluded == name))
    });
    binary_sources.sort();

    if binary_sources.is_empty() {
        panic!("user/src/bin 中没有用户程序源码");
    }
    if binary_sources.len() > MAX_USER_PROGRAMS {
        panic!("最多支持同时驻留 {MAX_USER_PROGRAMS} 个用户程序");
    }

    let mut layout = String::new();
    for (index, source_path) in binary_sources.iter().enumerate() {
        let program_name = source_path
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("用户程序文件名必须是有效 UTF-8");
        let slot_offset = index
            .checked_mul(USER_IMAGE_SLOT_SIZE)
            .expect("用户程序槽位偏移溢出");
        let link_address = USER_IMAGE_BASE
            .checked_add(slot_offset)
            .expect("用户程序链接地址溢出");

        // 每个 bin 都链接到它在运行时将被内核装入的固定槽位。
        println!(
            "cargo:rustc-link-arg-bin={program_name}=--defsym=__user_image_base={link_address:#x}"
        );
        println!("cargo:rustc-link-arg-bin={program_name}=-Tlinker.ld");
        layout.push_str(&format!("{program_name} {link_address:#x} 0x0\n"));
        println!("cargo:rerun-if-changed={}", source_path.display());
    }

    let requested_layout_path = env::var_os(LAYOUT_FILE_ENV).map(PathBuf::from);
    let layout_path = requested_layout_path.unwrap_or_else(|| {
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo 应提供 OUT_DIR"))
            .join("user-program-layout")
    });
    write_layout(&layout_path, layout.as_bytes());
}

fn binary_sources(directory: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!("无法读取用户程序目录 {}：{error}", directory.display());
    });

    entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("无法读取用户程序目录项：{error}"))
                .path()
        })
        .filter(|path| {
            let is_rust_bin =
                path.is_file() && path.extension().is_some_and(|extension| extension == "rs");
            let is_disabled = path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("disabled-"));
            is_rust_bin && !is_disabled
        })
        .collect()
}

fn write_layout(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("无法创建用户程序布局目录 {}：{error}", parent.display());
        });
    }

    let temporary_path = path.with_extension("tmp");
    fs::write(&temporary_path, contents).unwrap_or_else(|error| {
        panic!(
            "无法写入用户程序布局临时文件 {}：{error}",
            temporary_path.display()
        );
    });
    fs::rename(&temporary_path, path).unwrap_or_else(|error| {
        panic!("无法更新用户程序布局文件 {}：{error}", path.display());
    });
}
