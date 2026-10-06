use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const IMAGE_MANIFEST_ENV: &str = "FULLEMU_USER_IMAGE_MANIFEST";
const MAX_USER_PROGRAMS: usize = 8;

fn main() {
    println!("cargo:rerun-if-env-changed={IMAGE_MANIFEST_ENV}");

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
        if fields.len() != 2 {
            panic!(
                "用户程序镜像清单第 {} 行应包含程序名和镜像路径",
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
            .any(|(existing_name, _): &(&str, PathBuf)| *existing_name == name)
        {
            panic!("用户程序镜像清单中出现重复名称：{name}");
        }
        if entries.len() == MAX_USER_PROGRAMS {
            panic!("最多支持同时驻留 {MAX_USER_PROGRAMS} 个用户程序");
        }

        let relative_image_path = Path::new(fields[1]);
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
            "    UserProgramImage {{ name: {name:?}, bytes: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/user_program_{}.bin\")) }},\n",
            entries.len()
        ));
        entries.push((name, image_path));
    }

    if entries.is_empty() {
        panic!("指定的用户程序镜像清单中没有任何程序");
    }

    generated_catalog.push_str("];\n");
    generated_catalog
}
