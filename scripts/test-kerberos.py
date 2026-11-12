#!/usr/bin/env python3
"""Run native SSH tests with a private MIT Kerberos realm and loopback sshd.

Requires krb5-kdc, krb5-admin-server, krb5-user and openssh-server. No system
configuration, user credentials or services are changed; no tools are installed.
"""

import os
import pwd
import shutil
import socket
import subprocess
import time
from pathlib import Path

from fixture_support import fixture_directory


def tool(name):
    found = shutil.which(name) or shutil.which(name, path="/usr/sbin:/usr/bin")
    if not found:
        raise RuntimeError(f"Missing fixture prerequisite: {name}")
    return found


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_port(number, child):
    until = time.monotonic() + 10
    while time.monotonic() < until:
        if child.poll() is not None:
            raise RuntimeError("Fixture daemon exited before listening")
        try:
            with socket.create_connection(("127.0.0.1", number), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Fixture daemon did not start listening")


def main():
    os.chdir(Path(__file__).resolve().parent.parent)
    os.umask(0o077)
    tools = {
        name: tool(name)
        for name in ("krb5kdc", "kdb5_util", "kadmin.local", "kinit", "klist", "sshd", "ssh-keygen")
    }
    user = pwd.getpwuid(os.getuid()).pw_name
    children = []
    logs = []
    with fixture_directory(prefix="rode-kerberos-") as directory:
        root = Path(directory)
        env = os.environ.copy()
        # No caller cache, keytab, trace file or client keytab is consulted.
        for name in list(env):
            if name.startswith("KRB5"):
                del env[name]
        env.update(
            KRB5_CONFIG=str(root / "krb5.conf"),
            KRB5_KDC_PROFILE=str(root / "kdc.conf"),
            KRB5_KTNAME=f"FILE:{root}/host.keytab",
            KRB5RCACHENAME=f"file2:{root}/replay",
            KRB5CCNAME=f"FILE:{root}/valid.ccache",
        )
        kdc_port, ssh_port, second_port = port(), port(), port()
        (root / "krb5.conf").write_text(f"""[libdefaults]
 default_realm = RODE.TEST
 dns_lookup_kdc = false
 dns_lookup_realm = false
 dns_canonicalize_hostname = false
 rdns = false
 udp_preference_limit = 1
 default_client_keytab_name = FILE:{root}/missing-client.keytab
 default_rcache_name = file2:{root}/replay
[realms]
 RODE.TEST = {{
  kdc = 127.0.0.1:{kdc_port}
 }}
[domain_realm]
 127.0.0.1 = RODE.TEST
[logging]
 default = FILE:{root}/kerberos.log
""")
        plugin = os.environ.get("RODE_KRB5_DB_MODULE_DIR", "")
        (root / "kdc.conf").write_text(
            f"""[kdcdefaults]
 kdc_listen = 127.0.0.1:{kdc_port}
 kdc_tcp_listen = 127.0.0.1:{kdc_port}
[realms]
 RODE.TEST = {{
  database_name = {root}/principal
  key_stash_file = {root}/stash
  acl_file = {root}/acl
  max_life = 1d
  max_renewable_life = 7d
 }}
[dbmodules]
"""
            + (f" db_module_dir = {plugin}\n" if plugin else "")
        )
        (root / "acl").write_text("")

        def run(name, *args, **kwargs):
            return subprocess.run(
                [tools[name], *map(str, args)],
                env=env,
                check=True,
                timeout=30,
                capture_output=True,
                **kwargs,
            )

        def daemon(name, *args):
            log = (root / f"{name}-{len(children)}.log").open("wb")
            logs.append(log)
            child = subprocess.Popen(
                [tools[name], *map(str, args)],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
            )
            children.append(child)
            return child

        try:
            run("kdb5_util", "create", "-s", "-P", "fixture-only-master-key")
            for principal, keytab in [(user, "user.keytab"), ("host/127.0.0.1", "host.keytab")]:
                run("kadmin.local", "-q", f"addprinc -randkey {principal}")
                run("kadmin.local", "-q", f"ktadd -k {root / keytab} {principal}")
            wait_port(kdc_port, daemon("krb5kdc", "-n"))
            run("kinit", "-f", "-k", "-t", root / "user.keytab", "-l", "1h", user)
            run(
                "kinit",
                "-k",
                "-t",
                root / "user.keytab",
                "-l",
                "1s",
                "-c",
                f"FILE:{root}/expired.ccache",
                user,
            )
            for name in ["host", "client"]:
                run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", root / name)
            key = (root / "host.pub").read_text().split()
            (root / "known_hosts").write_text(
                "".join(f"[127.0.0.1]:{p} {key[0]} {key[1]}\n" for p in [ssh_port, second_port])
            )
            (root / "remote-home").mkdir()
            shell = root / "shell"
            shell.write_text(f"""#!/bin/sh
export HOME='{root}/remote-home'
export KRB5_CONFIG='{root}/krb5.conf'
exec /bin/sh -c "$SSH_ORIGINAL_COMMAND"
""")
            shell.chmod(0o700)
            for p, methods in [(ssh_port, "any"), (second_port, "gssapi-with-mic,publickey")]:
                config = root / f"sshd-{p}.conf"
                config.write_text(f"""Port {p}
ListenAddress 127.0.0.1
HostKey {root}/host
PidFile {root}/sshd-{p}.pid
AuthorizedKeysFile {root}/client.pub
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
GSSAPIAuthentication yes
GSSAPIStrictAcceptorCheck no
GSSAPICleanupCredentials yes
AuthenticationMethods {methods}
AllowUsers {user}
AllowTcpForwarding yes
LogLevel DEBUG3
ForceCommand {shell}
""")
                wait_port(p, daemon("sshd", "-D", "-e", "-f", config))
            (root / "ssh_config").write_text(f"""Host second-factor
 Port {second_port}
Host *
 HostName 127.0.0.1
 Port {ssh_port}
 User {user}
 IdentityFile {root}/client
 IdentityAgent none
 IdentitiesOnly yes
 UserKnownHostsFile {root}/known_hosts
 GlobalKnownHostsFile {root}/global_known_hosts
 StrictHostKeyChecking yes
 BatchMode yes
 GSSAPIAuthentication yes
 PasswordAuthentication no
 KbdInteractiveAuthentication no
""")
            time.sleep(2)
            expired = subprocess.run(
                [tools["klist"], "-s", "-c", f"FILE:{root}/expired.ccache"], env=env
            )
            if expired.returncode == 0:
                raise RuntimeError("Expired-ticket fixture is still valid")
            env.update(RODE_TEST_KERBEROS_DIR=str(root), RODE_TEST_KLIST=tools["klist"])
            if binary := env.get("RODE_KERBEROS_TEST_BINARY"):
                command = [binary, "--ignored", "--nocapture"]
            else:
                command = [
                    os.environ.get("CARGO", "cargo"),
                    "test",
                    "--locked",
                    "--test",
                    "ssh_kerberos",
                    "--",
                    "--ignored",
                    "--nocapture",
                ]
            subprocess.run(command, env=env, check=True, timeout=600)
        except Exception as error:
            if isinstance(error, subprocess.CalledProcessError):
                print(error.stdout or b"", error.stderr or b"")
            for log in root.glob("*.log"):
                print(f"{log.name}:\n{log.read_text(errors='replace')[-12000:]}")
            raise
        finally:
            for child in reversed(children):
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
