#!/usr/bin/env python3
"""Keep installed RODE helpers on the invoking machine out of the SSH fixture PATH."""

import os
import sys
from pathlib import Path


def isolated_path(path, destination):
    """Preserve PATH order and other tools, filtering only ambient helper executables.

    Mirror only directories containing rode-sessiond; leave interpreter and tool
    directories untouched otherwise. Symlink names retain tools such as rustup's
    cargo/rustc proxies without copying their binaries into the fixture.
    """
    result = []
    for index, entry in enumerate(path.split(os.pathsep)):
        directory = Path(entry or ".").absolute()
        if not (directory / "rode-sessiond").is_file():
            result.append(str(directory))
            continue
        mirror = Path(destination) / str(index)
        mirror.mkdir(parents=True, mode=0o700)
        for tool in directory.iterdir():
            if tool.name != "rode-sessiond" and tool.is_file() and os.access(tool, os.X_OK):
                (mirror / tool.name).symlink_to(tool)
        result.append(str(mirror))
    return os.pathsep.join(result)


if __name__ == "__main__":
    print(isolated_path(sys.argv[2], sys.argv[1]))
