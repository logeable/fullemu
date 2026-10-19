"""生成第一阶段块设备实验使用的单扇区原始镜像。"""

from pathlib import Path
import sys


SECTOR_SIZE = 512
SIGNATURE = b"FULLEMU-BLOCK-TEST\n"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("用法：create-block-test-image.py <输出镜像路径>")

    image_path = Path(sys.argv[1])
    image_path.parent.mkdir(parents=True, exist_ok=True)
    remaining_size = SECTOR_SIZE - len(SIGNATURE)
    sector = SIGNATURE + bytes(index & 0xFF for index in range(remaining_size))
    image_path.write_bytes(sector)


if __name__ == "__main__":
    main()
