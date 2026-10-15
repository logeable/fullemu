fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    // 所有用户程序使用同一链接脚本和虚拟入口；具体构建哪些 bin 由 Cargo 命令决定。
    println!("cargo:rustc-link-arg=-Tlinker.ld");
}
