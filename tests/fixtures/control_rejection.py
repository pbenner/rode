"""Negative session-control probes: require a framed error, including after an early close."""

import json
import os
import socket
import struct
import sys


def read_frame(client):
    """Read a complete bounded frame; EOF/reset without a denial remains a test failure."""

    def read_exact(size):
        result = b""
        while len(result) < size:
            chunk = client.recv(size - len(result))
            assert chunk, "Control peer closed without a complete rejection"
            result += chunk
        return result

    size = struct.unpack(">I", read_exact(4))[0]
    assert 0 < size <= 4 * 1024 * 1024, f"Invalid control frame size: {size}"
    return read_exact(size)


def assert_rejected(client, request, expected):
    """Accept an early write-side close only when the peer supplies the expected explicit error."""
    try:
        client.sendall(struct.pack(">I", len(request)) + request)
    except (BrokenPipeError, ConnectionResetError):
        # Discovery/namespace checks may reject before reading any bytes. The
        # queued denial must still be readable; a transport error proves nothing.
        pass
    frame = read_frame(client)
    assert frame[:1] == b"J", f"Unexpected control frame kind: {frame[:1]!r}"
    reply = json.loads(frame[1:])
    assert reply.get("kind") == "error", f"Control request was not rejected: {reply!r}"
    assert isinstance(reply.get("data"), str) and expected in reply["data"], reply


def probe_landlock(path, expose_runtime):
    """Check token privacy and both control endpoints inside an actual Landlock domain."""
    reference = os.path.join(os.path.dirname(path), "control.capability")
    if expose_runtime:
        # The file contains only a reference. The host's ptrace-protected
        # descriptor must remain inaccessible even with an explicit path grant.
        with open(reference) as handle:
            reference = handle.read()
    try:
        with open(reference, "rb") as handle:
            handle.read()
    except PermissionError:
        pass
    else:
        raise AssertionError("Landlock read host control capability")
    metadata = os.stat(os.path.dirname(path))
    address = "\0rode-session-auth-%s-%s-%s" % (metadata.st_uid, metadata.st_dev, metadata.st_ino)
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        try:
            client.connect(address)
        except PermissionError:
            pass  # ABI 6 can block the connection itself.
        else:  # ABI 4/5 rely on the ptrace-protected capability.
            assert_rejected(client, b"A" + b"0" * 32, "authentication failed")
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(path)
        assert_rejected(client, b'J{"action":"list"}', "outside the project sandbox")


if __name__ == "__main__":
    probe_landlock(sys.argv[1], sys.argv[2] == "true")
