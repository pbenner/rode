"""One-shot API v1 adapters; called only through the active project policy.

Prepended to files::REMOTE_HELPER; uses project_run and shared imports from those peers. The
manifest is validated on the desktop and adapters start only for explicit, enabled operations.
The enclosing project policy is authoritative; no local-host fallback, automatic installation
or persistent adapter daemon is created.
"""


def extension_run(root, invocation, cancel=None):
    """Execute one bounded API v1 adapter operation and validate its result."""
    if invocation.get("api") != 1:
        raise ValueError("Unsupported adapter API.")
    payload = dict(
        api=1,
        extension=invocation["extension"],
        operation=invocation["operation"],
        root=root,
        path=invocation.get("path"),
        text=invocation["text"],
        settings=invocation.get("settings", {}),
    )
    result = project_checked(  # noqa: F821 - concatenated peer
        project_run(  # noqa: F821
            [invocation["program"], *invocation["args"]],
            root,
            json.dumps(payload),  # noqa: F821
            timeout=20,
            cancel=cancel,
        )
    )
    reply = json.loads(result)  # noqa: F821
    if reply.get("api") != 1:
        raise ValueError("Unsupported adapter response API.")
    return dict(kind="extension", data=reply)


def extension_coordinator():
    """Read a captured adapter request and honor cancellation through the coordinator input channel."""
    request = json.loads(sys.stdin.buffer.readline(64 * 1024 * 1024))  # noqa: F821
    cancel = threading.Event()  # noqa: F821

    def control():
        # Avoid holding Python's buffered-stdin lock during normal shutdown.
        """Cancel the adapter when the control channel receives a byte or closes, avoiding buffered-stdin locks."""
        os.read(sys.stdin.fileno(), 1)  # noqa: F821
        cancel.set()

    threading.Thread(target=control, daemon=True).start()  # noqa: F821
    root = os.path.realpath(os.path.expanduser(request["root"]))  # noqa: F821
    return extension_run(root, request["invocation"], cancel)["data"]
