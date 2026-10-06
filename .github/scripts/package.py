#!/usr/bin/env python3
"""Package a release build: dist/didcomm-mcp-<target>.tar.gz (.zip on Windows) holding
didcomm-mcp-<target>/{didcomm-mcp[.exe], README.md, INSTALL.md, LICENSE}, plus its
SHA-256 in dist/didcomm-mcp-<target>.<ext>.sha256 (sha256sum format).

  package.py <target>
"""

import hashlib
import io
import os
import sys
import tarfile
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def main():
    target = sys.argv[1]
    windows = "windows" in target
    exe = "didcomm-mcp.exe" if windows else "didcomm-mcp"
    name = f"didcomm-mcp-{target}"
    files = [
        (os.path.join(ROOT, "target", target, "release", exe), exe, 0o755),
        (os.path.join(ROOT, "README.md"), "README.md", 0o644),
        (os.path.join(ROOT, "docs", "install.md"), "INSTALL.md", 0o644),
        (os.path.join(ROOT, "LICENSE"), "LICENSE", 0o644),
    ]
    dist = os.path.join(ROOT, "dist")
    os.makedirs(dist, exist_ok=True)
    archive = os.path.join(dist, f"{name}.{'zip' if windows else 'tar.gz'}")

    if windows:
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
            for source, inside, _ in files:
                z.write(source, f"{name}/{inside}")
    else:
        with tarfile.open(archive, "w:gz") as t:
            for source, inside, mode in files:
                data = open(source, "rb").read()
                info = tarfile.TarInfo(f"{name}/{inside}")
                info.size, info.mode, info.mtime = len(data), mode, int(os.path.getmtime(source))
                t.addfile(info, io.BytesIO(data))

    digest = hashlib.sha256(open(archive, "rb").read()).hexdigest()
    with open(archive + ".sha256", "w", newline="\n") as f:
        f.write(f"{digest}  {os.path.basename(archive)}\n")
    print(f"{os.path.basename(archive)}  {os.path.getsize(archive)} bytes  sha256 {digest}")


if __name__ == "__main__":
    main()
