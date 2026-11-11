#!/usr/bin/env python3
"""Private SSH configuration, portable shell probes and isolated fixture tools."""

import argparse
import json
import os
import shlex
from pathlib import Path


def config_token(value):
    """Quote one OpenSSH config token; reject line boundaries instead of injecting directives."""
    value = str(value)
    if any(char in value for char in "\r\n\0"):
        raise ValueError("SSH fixture config values must fit on one line")
    return json.dumps(value, ensure_ascii=False)


def script(path, source):
    """Write one executable into the already-private fixture directory."""
    path.write_text("#!/bin/sh\n" + source, encoding="utf-8")
    path.chmod(0o700)


def configure(root, *, search_path, project_shell, xauth, port, user, x11="yes"):
    """Generate only inside the caller's private directory; keys remain owned by test-ssh.sh."""
    port = int(port)
    if not 1 <= port <= 65535 or x11 not in {"yes", "no"}:
        raise ValueError("Invalid SSH fixture port or X11 mode")
    root = Path(root).resolve()
    exports = dict(
        HOME=str(root / "home"),
        PATH=search_path,
        TMPDIR=str(root),
        RODE_SESSION_DIR=str(root / "home/sessions"),
        RODE_VAULT_DIR=str(root / "home/vaults"),
        XAUTHORITY=str(root / "home/.Xauthority"),
    )
    if project_shell:
        exports["SHELL"] = project_shell
    script(
        root / "host-shell",
        "".join(f"export {name}={shlex.quote(value)}\n" for name, value in exports.items())
        + 'if [ -n "${SSH_ORIGINAL_COMMAND:-}" ]; then\n'
        '    exec /bin/sh -c "$SSH_ORIGINAL_COMMAND"\nfi\nexec /bin/sh\n',
    )
    server_x11 = "X11Forwarding no\n"
    client_xauth = ""
    if xauth:
        script(
            root / "xauth",
            "exec "
            + shlex.quote(xauth)
            + " -f "
            + shlex.quote(str(root / "home/.Xauthority"))
            + ' "$@"\n',
        )
        server_x11 = (
            f"X11Forwarding {x11}\nX11UseLocalhost yes\n"
            f"XAuthLocation {config_token(root / 'xauth')}\n"
        )
        client_xauth = f"    XAuthLocation {config_token(xauth)}\n"
    server = (
        f"Port {port}\nListenAddress 127.0.0.1\n"
        f"HostKey {config_token(root / 'host')}\nHostKey {config_token(root / 'host-known')}\n"
        f"PidFile {config_token(root / 'sshd.pid')}\n"
        f"AuthorizedKeysFile {config_token(root / 'client.pub')}\n"
        "StrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
        f"UsePAM no\nPermitUserRC no\nPermitUserEnvironment no\nUseDNS no\nAllowUsers {config_token(user)}\nLogLevel VERBOSE\n"
        "GatewayPorts clientspecified\nMaxStartups 64\n"
        + server_x11
        + "AcceptEnv CODEX_HOME RODE_CODEX_CACHE\n"
        f"SetEnv {config_token('HOME=' + str(root / 'home'))}\n"
        f"SetEnv {config_token('PATH=' + search_path)}\n"
        f"ForceCommand {shlex.quote(str(root / 'host-shell'))}\n"
    )
    (root / "sshd_config").write_text(server)
    key_type, key_data, *_ = (root / "host-known.pub").read_text().split()
    (root / "known_hosts").write_text(f"[127.0.0.1]:{port} {key_type} {key_data}\n")
    (root / "global_known_hosts").write_text("")
    client = (
        "Host rode-test rode-jump\n    HostName 127.0.0.1\n"
        f"    User {config_token(user)}\n    Port {port}\n"
        f"    IdentityFile {config_token(root / 'client')}\n    IdentitiesOnly yes\n"
        f"    UserKnownHostsFile {config_token(root / 'known_hosts')}\n"
        f"    GlobalKnownHostsFile {config_token(root / 'global_known_hosts')}\n"
        "    StrictHostKeyChecking yes\n    BatchMode yes\n    GSSAPIAuthentication yes\n"
        + client_xauth
        + f"    SetEnv {config_token('CODEX_HOME=' + str(root / 'codex-home'))} "
        f"{config_token('RODE_CODEX_CACHE=' + str(root / 'codex-cache'))}\n"
    )
    (root / "ssh_config").write_text(client)


def shell_probe(marker):
    """Source the access probe in the existing shell, including shells without dot arguments."""
    return f"set -- {shlex.quote(marker)}; . ./check-access\n"


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


def main():
    """Accept literal fixture arguments without evaluating shell or config contents."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    isolated = commands.add_parser("path", help="Hide ambient helpers while retaining other tools")
    isolated.add_argument("destination")
    isolated.add_argument("search_path")
    config = commands.add_parser("configure", help="Write private shell and SSH configuration")
    for name in ("root", "search_path", "project_shell", "xauth", "port", "user", "x11"):
        config.add_argument(name)
    values = vars(parser.parse_args())
    if values.pop("command") == "path":
        print(isolated_path(values["search_path"], values["destination"]))
    else:
        configure(values.pop("root"), **values)


if __name__ == "__main__":
    main()
