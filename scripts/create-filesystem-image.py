"""创建初始文件系统使用的空白 raw 磁盘镜像。"""

from pathlib import Path
import sys


IMAGE_SIZE = 16 * 1024 * 1024


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("用法：create-filesystem-image.py <输出镜像路径>")

    image_path = Path(sys.argv[1])
    image_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with image_path.open("xb") as image_file:
            image_file.truncate(IMAGE_SIZE)
    except FileExistsError as error:
        raise SystemExit(f"拒绝覆盖已经存在的镜像文件：{image_path}") from error


if __name__ == "__main__":
    main()
