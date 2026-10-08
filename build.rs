use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const IMAGE_MANIFEST_ENV: &str = "FULLEMU_USER_IMAGE_MANIFEST";
const BOOT_PROGRAM_ENV: &str = "FULLEMU_BOOT_PROGRAM";
const BOOT_MODE_ENV: &str = "FULLEMU_BOOT_MODE";
const USER_PROGRAM_VIRTUAL_BASE: usize = 0x8040_0000;
const MAX_USER_PROGRAMS: usize = 8;

fn main() {
    println!("cargo:rustc-link-arg-bin=fullemu=-Tlinker.ld");
    println!("cargo:rerun-if-env-changed={IMAGE_MANIFEST_ENV}");
    println!("cargo:rerun-if-env-changed={BOOT_PROGRAM_ENV}");
    println!("cargo:rerun-if-env-changed={BOOT_MODE_ENV}");

    let boot_mode = env::var(BOOT_MODE_ENV).unwrap_or_else(|_| "shell".to_owned());
    if boot_mode != "shell" && boot_mode != "batch" {
        panic!("{BOOT_MODE_ENV} 只支持 shell 或 batch，当前值为：{boot_mode}");
    }
    println!("cargo:rustc-env={BOOT_MODE_ENV}={boot_mode}");

    let output_directory =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo 应为构建脚本提供 OUT_DIR"));
    let generated_source = output_directory.join("user_program_catalog.rs");
    let manifest_path = env::var_os(IMAGE_MANIFEST_ENV).map(PathBuf::from);

    let generated_catalog = match manifest_path {
        Some(path) if !path.as_os_str().is_empty() => generate_catalog(&path, &output_directory),
        _ => {
            println!("cargo:warning=未提供用户程序镜像清单，内核将不嵌入用户程序");
            "const USER_PROGRAMS: &[UserProgramImage] = &[];\n".to_owned()
        }
    };

    fs::write(generated_source, generated_catalog).unwrap_or_else(|error| {
        panic!("无法写入生成的用户程序清单：{error}");
    });
}

fn generate_catalog(manifest_path: &Path, output_directory: &Path) -> String {
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    let manifest = fs::read_to_string(manifest_path).unwrap_or_else(|error| {
        panic!(
            "无法读取用户程序镜像清单 {}：{error}",
            manifest_path.display()
        );
    });
    let workspace_directory = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo 应为构建脚本提供 CARGO_MANIFEST_DIR"),
    );
    let mut entries = Vec::new();
    let mut generated_catalog = String::from("const USER_PROGRAMS: &[UserProgramImage] = &[\n");

    for (line_index, line) in manifest.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }

        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 {
            panic!(
                "用户程序镜像清单第 {} 行应包含程序名、用户虚拟地址和镜像路径",
                line_index + 1
            );
        }

        let name = fields[0];
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            panic!(
                "用户程序名只能包含 ASCII 字母、数字、下划线和连字符（第 {} 行）",
                line_index + 1
            );
        }
        if entries
            .iter()
            .any(|existing_name: &&str| *existing_name == name)
        {
            panic!("用户程序镜像清单中出现重复名称：{name}");
        }
        if entries.len() == MAX_USER_PROGRAMS {
            panic!("最多支持同时驻留 {MAX_USER_PROGRAMS} 个用户程序");
        }

        let linked_base = parse_hexadecimal(fields[1], "用户虚拟地址", line_index + 1);
        if linked_base != USER_PROGRAM_VIRTUAL_BASE {
            panic!(
                "用户程序 {name} 的虚拟地址为 {linked_base:#x}，但所有程序必须链接到 {USER_PROGRAM_VIRTUAL_BASE:#x}（第 {} 行）",
                line_index + 1
            );
        }

        let relative_image_path = Path::new(fields[2]);
        let image_path = if relative_image_path.is_absolute() {
            relative_image_path.to_path_buf()
        } else {
            workspace_directory.join(relative_image_path)
        };
        if !image_path.is_file() {
            panic!(
                "用户程序镜像不存在：{}（清单第 {} 行）",
                image_path.display(),
                line_index + 1
            );
        }

        let image_bytes = fs::read(&image_path).unwrap_or_else(|error| {
            panic!("无法读取用户程序镜像 {}：{error}", image_path.display());
        });
        if image_bytes.is_empty() {
            panic!("用户程序镜像不能为空：{}", image_path.display());
        }
        let copied_image_path =
            output_directory.join(format!("user_program_{}.bin", entries.len()));
        fs::write(&copied_image_path, image_bytes).unwrap_or_else(|error| {
            panic!(
                "无法复制用户程序镜像到 {}：{error}",
                copied_image_path.display()
            );
        });
        println!("cargo:rerun-if-changed={}", image_path.display());

        generated_catalog.push_str(&format!(
            "    UserProgramImage {{ name: {name:?}, linked_base: {linked_base:#x}, bytes: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/user_program_{}.bin\")) }},\n",
            entries.len(),
        ));
        entries.push(name);
    }

    if entries.is_empty() {
        panic!("指定的用户程序镜像清单中没有任何程序");
    }

    generated_catalog.push_str("];\n");
    generated_catalog
}

fn parse_hexadecimal(value: &str, field_name: &str, line_number: usize) -> usize {
    let digits = value.strip_prefix("0x").unwrap_or(value);
    usize::from_str_radix(digits, 16).unwrap_or_else(|_| {
        panic!("用户程序镜像清单第 {line_number} 行的{field_name}不是有效十六进制数：{value}");
    })
}
