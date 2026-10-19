"""生成 VirtIO 块设备读写实验使用的双扇区原始镜像。"""

from pathlib import Path
import sys


SECTOR_SIZE = 512
SIGNATURE = b"FULLEMU-BLOCK-TEST\n"
SCRATCH_SIGNATURE = b"FULLEMU-BLOCK-SCRATCH\n"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("用法：create-block-test-image.py <输出镜像路径>")

    image_path = Path(sys.argv[1])

    def make_sector(signature: bytes, multiplier: int, increment: int) -> bytes:
        return signature + bytes(
            (index * multiplier + increment) & 0xFF
            for index in range(SECTOR_SIZE - len(signature))
        )

    image_path.parent.mkdir(parents=True, exist_ok=True)
    image = make_sector(SIGNATURE, 1, 0) + make_sector(SCRATCH_SIGNATURE, 3, 7)
    try:
        with image_path.open("xb") as image_file:
            image_file.write(image)
    except FileExistsError as error:
        raise SystemExit(f"拒绝覆盖已经存在的镜像文件：{image_path}") from error


if __name__ == "__main__":
    main()
