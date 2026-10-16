"""检查当前内核 ELF 的启动入口和物理/虚拟加载地址基线。"""

import json
import subprocess
import sys


BOOT_ADDRESS = 0x80200000
PAGE_SIZE = 4096


def main() -> int:
    if len(sys.argv) != 3:
        print("用法：check_kernel_elf.py <内核 ELF 路径> <rust-readobj 命令>", file=sys.stderr)
        return 2

    elf_path, readobj = sys.argv[1:]
    try:
        output = subprocess.run(
            [readobj, "--elf-output-style=JSON", "--file-header", "--program-headers", elf_path],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except FileNotFoundError:
        print(f"找不到 ELF 检查工具：{readobj}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as error:
        print(error.stderr, file=sys.stderr, end="")
        return error.returncode

    try:
        document = json.loads(output)[0]
        header = document["ElfHeader"]
        entry = header["Entry"]
        load_segments = [
            item["ProgramHeader"]
            for item in document["ProgramHeaders"]
            if item["ProgramHeader"]["Type"]["Name"] == "PT_LOAD"
        ]
    except (KeyError, IndexError, TypeError, json.JSONDecodeError) as error:
        print(f"无法读取 rust-readobj 的 ELF 信息：{error}", file=sys.stderr)
        return 2

    errors = []
    if header["Machine"]["Name"] != "EM_RISCV":
        errors.append("内核 ELF 目标架构不是 RISC-V")
    if entry != BOOT_ADDRESS:
        errors.append(f"入口应为低地址启动入口 {BOOT_ADDRESS:#x}，实际为 {entry:#x}")
    if not load_segments:
        errors.append("ELF 中没有 PT_LOAD 段")
    elif load_segments[0]["VirtualAddress"] != BOOT_ADDRESS:
        errors.append("第一个 PT_LOAD 段没有从启动入口地址开始")
    elif not (
        load_segments[0]["VirtualAddress"]
        <= entry
        < load_segments[0]["VirtualAddress"] + load_segments[0]["MemSize"]
    ):
        errors.append("ELF 入口不在第一个 PT_LOAD 段的内存范围内")

    for index, segment in enumerate(load_segments):
        virtual = segment["VirtualAddress"]
        physical = segment["PhysicalAddress"]
        alignment = segment["Alignment"]
        flags = {flag["Name"] for flag in segment["Flags"]["Flags"]}
        if virtual != physical:
            errors.append(
                f"PT_LOAD[{index}] 当前基线要求 VMA 与 LMA 相同，"
                f"实际为 {virtual:#x} 与 {physical:#x}"
            )
        if alignment < PAGE_SIZE:
            errors.append(f"PT_LOAD[{index}] 对齐小于一页：{alignment:#x}")
        if index == 0 and "PF_X" not in flags:
            errors.append("第一个 PT_LOAD 段不可执行，启动入口无法从该段取指")

    print(f"内核 ELF：{elf_path}")
    print(f"入口地址：{entry:#018x}")
    for index, segment in enumerate(load_segments):
        flags = "".join(
            letter
            for name, letter in (("PF_R", "R"), ("PF_W", "W"), ("PF_X", "X"))
            if any(flag["Name"] == name for flag in segment["Flags"]["Flags"])
        )
        print(
            f"PT_LOAD[{index}]：VMA={segment['VirtualAddress']:#018x} "
            f"LMA={segment['PhysicalAddress']:#018x} "
            f"文件={segment['FileSize']:#x} 内存={segment['MemSize']:#x} "
            f"权限={flags} 对齐={segment['Alignment']:#x}"
        )

    if errors:
        for error in errors:
            print(f"布局错误：{error}", file=sys.stderr)
        return 1

    print("当前基线有效：入口和所有可加载段都使用低地址恒等布局。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
