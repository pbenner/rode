"""Check explicit device access and unrelated-file denial in a sandbox fixture."""

import errno
import fcntl
import os
import sys

device, expected, secret = sys.argv[1:]
try:
    descriptor = os.open(device, os.O_RDWR)
except OSError as error:
    assert expected == "denied" and error.errno in (
        errno.EACCES,
        errno.EPERM,
        errno.ENOENT,
    ), f"Expected {expected} device access at {device}: {error}"
else:
    try:
        assert expected == "allowed", f"Unexpected device access at {device}"
        assert os.read(descriptor, 1) == bytes(1), "Device did not return a zero byte"
        try:
            fcntl.ioctl(descriptor, 0xDEADBEEF, 0)
        except OSError as error:
            assert error.errno == errno.ENOTTY, f"Device ioctl was denied: {error}"
    finally:
        os.close(descriptor)
try:
    with open(secret, "rb"):
        pass
except (PermissionError, FileNotFoundError):
    pass
else:
    raise AssertionError("Device grant exposed unrelated files")
