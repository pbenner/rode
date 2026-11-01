"""Start a named helper from streamed source, preserving unread protocol input.

The enclosing loader supplies script_name and script_source. The private script
removes itself before running the original source, so neither caches nor a remote
RODE installation are needed. exec preserves PID, process group and SSH lifetime.
"""

import os
import sys
import tempfile

script_name = globals()["script_name"]
script_source = globals()["script_source"]
directory = tempfile.mkdtemp(prefix="rode-python-")
path = os.path.join(directory, script_name)
try:
    # Execute the original source separately to preserve future imports and its
    # own line numbers. Restore -c's import-path behavior after moving to a file.
    wrapper = (
        "import os as _rode_os, sys as _rode_sys\n"
        "_rode_os.unlink(__file__)\n"
        "_rode_os.rmdir(_rode_os.path.dirname(__file__))\n"
        "_rode_sys.path[0] = ''\n"
        "del _rode_os, _rode_sys\n"
        f"exec(compile({script_source!r}, {script_name!r}, 'exec'))\n"
    )
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        stream.write(wrapper)
    os.execv(sys.executable, [sys.executable, "-u", path, *sys.argv[2:]])
finally:
    # exec failures must not leave the staged script behind either.
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    os.rmdir(directory)
