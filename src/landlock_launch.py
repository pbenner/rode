"""Shared local/SSH Landlock launcher; Python 3 standard library only.

Receives RODE's literal mount-policy arguments, then execs the command. This
implements access rules, not mounts: aliases are rejected, as are restrictive
child grants that additive Landlock rules cannot express. No failure retries
the command without enforcement. Linux UAPI: Landlock ABI 6 and seccomp BPF.
"""

import contextlib
import ctypes
import errno
import hashlib
import os
import platform
import signal
import stat
import sys


def checked(result, operation):
    if result < 0:
        error = ctypes.get_errno()
        raise OSError(error, operation + ": " + os.strerror(error))
    return result


def open_captured_path(path):
    """Pin a prepared absolute path without following any newly introduced link.

    O_NOFOLLOW on just the last component is insufficient: a writable ancestor
    could have been replaced between launches or while this walk runs.
    """
    if not os.path.isabs(path) or os.path.normpath(path) != path:
        raise ValueError("Landlock requires a canonical captured path: " + path)
    descriptor = os.open("/", os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        components = [part for part in path.split("/") if part]
        for index, component in enumerate(components):
            flags = os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC
            if index < len(components) - 1:
                flags |= os.O_DIRECTORY
            child = os.open(component, flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
            if stat.S_ISLNK(os.fstat(descriptor).st_mode):
                raise ValueError("Landlock grant changed to a symlink: " + path)
        result, descriptor = descriptor, None
        return result
    finally:
        if descriptor is not None:
            os.close(descriptor)


def private_scratch(home, root):
    """Stable per-project scratch survives persisted sessions without sharing /tmp."""
    key = hashlib.sha256(os.fsencode(root)).hexdigest()
    path, descriptor = home, open_captured_path(home)
    try:
        for component in (".cache", "rode", "landlock", key):
            path = os.path.join(path, component)
            try:
                os.mkdir(component, 0o700, dir_fd=descriptor)
            except FileExistsError:
                pass
            child = os.open(
                component,
                os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                dir_fd=descriptor,
            )
            os.close(descriptor)
            descriptor = child
            metadata = os.fstat(descriptor)
            if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
                raise ValueError("Unsafe Landlock scratch directory: " + path)
            # Existing ~/.cache may be public; RODE-owned descendants must be private.
            if component != ".cache" and metadata.st_mode & 0o077:
                raise ValueError("Landlock scratch directory must be owner-only: " + path)
        result, descriptor = descriptor, None
        return path, result
    finally:
        if descriptor is not None:
            os.close(descriptor)


def validate_grants(grants, hidden):
    """Additive rules cannot subtract a protected child from a broader grant."""
    for child, writable in grants + [(path, False) for path in hidden]:
        for parent, parent_writable in grants:
            if os.path.commonpath((parent, child)) == parent and (
                child in hidden or (parent_writable and not writable)
            ):
                raise ValueError(
                    "Landlock cannot enforce a restrictive nested grant: "
                    + child
                    + " inside "
                    + parent
                    + "; use bwrap or remove the broader grant"
                )

    # Python and its standard library execute before restrictions take effect.
    # A project must not be allowed to replace any of that launcher's code for
    # the next invocation. -I -S excludes project/site and PYTHONPATH imports.
    runtime_paths = {os.path.realpath(path) for path in [sys.executable, *sys.path] if path}
    for runtime in runtime_paths:
        for path, writable in grants:
            if writable and os.path.commonpath((runtime, path)) in (runtime, path):
                raise ValueError("Landlock interpreter/runtime overlaps a writable grant: " + path)


def policy(arguments):
    """Consume only the known shared policy vocabulary, never arbitrary bwrap flags."""
    grants, hidden, environment = [], [], {}
    network, root, index = False, None, 0
    policy_home = None
    while index < len(arguments):
        flag = arguments[index]
        index += 1
        if flag == "--":
            break
        if flag in ("--unshare-all", "--die-with-parent"):
            continue
        if flag == "--share-net":
            network = True
        elif flag in ("--ro-bind", "--bind"):
            source, destination = arguments[index : index + 2]
            index += 2
            if source != destination:
                raise ValueError("Landlock cannot remap filesystem paths")
            grants.append((source, flag == "--bind"))
        elif flag == "--rode-home":
            policy_home = arguments[index]
            index += 1
        elif flag == "--rode-deny":
            hidden.append(os.path.realpath(arguments[index]))
            index += 1
        elif flag == "--setenv":
            name, value = arguments[index : index + 2]
            environment[name] = value
            index += 2
        elif flag in ("--tmpfs", "--dir", "--proc", "--dev", "--chdir"):
            path = arguments[index]
            index += 1
            if flag == "--chdir":
                root = path
            elif flag == "--proc" and path == "/proc":
                grants.append((path, False))
            elif flag == "--dev" and path == "/dev":
                for device in ("null", "zero", "random", "urandom", "tty", "ptmx", "pts"):
                    device = os.path.realpath("/dev/" + device)
                    if os.path.exists(device):
                        grants.append((device, True))
            elif flag == "--tmpfs" and path not in ("/tmp", "/home"):
                raise ValueError("Unsupported Landlock temporary location")
            elif flag not in ("--tmpfs", "--dir"):
                raise ValueError("Unsupported Landlock device/process view")
        else:
            raise ValueError("Unsupported Landlock policy flag: " + flag)
    else:
        raise ValueError("Missing Landlock command boundary")
    if not root or not policy_home or not arguments[index:]:
        raise ValueError("Incomplete Landlock launch policy")
    # A union of grants cannot subtract a child from a parent. Refuse this
    # before opening any rule descriptors, including aliases such as /bin.
    validate_grants(grants, hidden)
    return grants, hidden, environment, network, root, arguments[index:], policy_home


def restrict_network(libc, machine):
    """Deny socket creation/use and io_uring; include UDP/Unix, not just TCP.

    Architecture checks prevent compat/x32 syscall-number bypasses. Existing
    stdin/out/err remain the explicitly inherited editor transport capability.
    """
    if machine == "x86_64":
        audit_arch = 0xC000003E
        denied = (41, 42, 43, 44, 46, 49, 50, 288, 307, 425, 426, 427)
    else:  # aarch64, checked before any syscall
        audit_arch = 0xC00000B7
        denied = (198, 200, 201, 202, 203, 206, 211, 242, 269, 425, 426, 427)

    class Filter(ctypes.Structure):
        _fields_ = [
            ("code", ctypes.c_ushort),
            ("jt", ctypes.c_ubyte),
            ("jf", ctypes.c_ubyte),
            ("k", ctypes.c_uint),
        ]

    class Program(ctypes.Structure):
        _fields_ = [("length", ctypes.c_ushort), ("filter", ctypes.POINTER(Filter))]

    instructions = [
        (0x20, 0, 0, 4),  # load seccomp_data.arch
        (0x15, 1, 0, audit_arch),
        (0x06, 0, 0, 0x80000000),  # kill mismatched ABI
        (0x20, 0, 0, 0),  # load syscall number
        (0x35, 0, 1, 0x40000000),  # reject x32/negative syscall numbers
        (0x06, 0, 0, 0x80000000),
    ]
    for syscall in denied:
        instructions += [(0x15, 0, 1, syscall), (0x06, 0, 0, 0x00050000 | errno.EPERM)]
    instructions.append((0x06, 0, 0, 0x7FFF0000))
    filters = (Filter * len(instructions))(*(Filter(*row) for row in instructions))
    program = Program(len(filters), filters)
    checked(libc.prctl(22, 2, ctypes.byref(program), 0, 0), "seccomp network policy")


def main():
    parent_pid = os.getppid()
    machine = platform.machine()
    if sys.platform != "linux" or machine not in ("x86_64", "aarch64"):
        raise ValueError("Landlock requires Linux x86_64 or aarch64")
    if ctypes.sizeof(ctypes.c_void_p) != 8:
        raise ValueError("Landlock requires a 64-bit Python interpreter")
    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    # These numbers are shared by the two explicitly supported Linux ABIs.
    abi = checked(libc.syscall(444, None, ctypes.c_size_t(0), 1), "Landlock ABI probe")
    if abi < 6:
        raise ValueError("Landlock ABI 6 or newer is required; host provides ABI " + str(abi))
    grants, hidden, environment, network, root, command, home = policy(sys.argv[1:])

    class Ruleset(ctypes.Structure):
        _fields_ = [
            ("filesystem", ctypes.c_uint64),
            ("network", ctypes.c_uint64),
            ("scoped", ctypes.c_uint64),
        ]

    class PathRule(ctypes.Structure):
        _pack_ = 1
        _fields_ = [("access", ctypes.c_uint64), ("parent_fd", ctypes.c_int32)]

    all_rights = (1 << 16) - 1  # ABI 5 IOCTL_DEV, including REFER/TRUNCATE
    read_rights = (1 << 0) | (1 << 2) | (1 << 3)
    file_rights = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 14) | (1 << 15)
    ruleset = Ruleset(all_rights, 0 if network else 3, 3)
    with contextlib.ExitStack() as descriptors:
        scratch, scratch_fd = private_scratch(home, root)
        descriptors.callback(os.close, scratch_fd)
        grants.append((scratch, True))
        # Scratch is an effective writable grant too; it must not mask a more
        # specific user restriction, including a read-only rule on scratch itself.
        validate_grants(grants, hidden)
        environment.update(TMPDIR=scratch, TMP=scratch, TEMP=scratch)
        descriptor = checked(
            libc.syscall(444, ctypes.byref(ruleset), ctypes.sizeof(ruleset), 0),
            "Landlock ruleset",
        )
        descriptors.callback(os.close, descriptor)
        root_fd = None
        for path, writable in grants:
            parent = scratch_fd if path == scratch else open_captured_path(path)
            if parent != scratch_fd:
                descriptors.callback(os.close, parent)
            if path == root:
                root_fd = parent
            access = all_rights if writable else read_rights
            if not stat.S_ISDIR(os.fstat(parent).st_mode):
                access &= file_rights
            rule = PathRule(access, parent)
            checked(
                libc.syscall(445, descriptor, 1, ctypes.byref(rule), 0), "Landlock grant " + path
            )
        if root_fd is None:
            raise ValueError("Landlock project root must have an explicit grant")
        checked(libc.prctl(38, 1, 0, 0, 0), "no_new_privs")
        checked(libc.syscall(446, descriptor, 0), "Landlock enforcement")
        os.fchdir(root_fd)
    if not network:
        restrict_network(libc, machine)
    # Do not leak pre-sandbox directory/file/socket capabilities to the command.
    # An inherited descriptor can exceed a subsequently lowered RLIMIT_NOFILE.
    # close_range (Linux 5.9+) closes those too, without a Python-level scan.
    checked(
        libc.syscall(436, ctypes.c_uint(3), ctypes.c_uint(0xFFFFFFFF), 0),
        "close inherited descriptors",
    )
    checked(libc.prctl(1, signal.SIGKILL, 0, 0, 0), "parent lifetime")
    if os.getppid() != parent_pid:
        raise RuntimeError("Landlock launch parent exited")
    os.environ.update(environment)
    os.execvpe(command[0], command, os.environ)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("Landlock: " + str(error), file=sys.stderr)
        sys.exit(1)
