# Remote connections

RODE uses an embedded Rust SSH client (`russh`) by default. The desktop needs no
`ssh`, `scp` or `ssh-keygen` executable for connections, helper deployment, file
operations or project terminals. The remote machine still needs a running SSH
server and a POSIX shell. External programs such as Git-over-SSH retain their own
client requirements.

Click **Connect SSH**. Every connection can configure:

- Client backend: **Rust (default)** or **OpenSSH**
- SSH config alias, hostname, or `user@hostname`
- Username, port, identity file, and remote folder
- One or more comma-separated ProxyJump hosts
- An alternative SSH config file
- An optional preinstalled remote helper path
- X11 forwarding when available (enabled by default)
- An optional **Connection name**, such as `Work cluster` or `Home server`, shown
  in the Explorer and connection picker instead of the SSH address

Saved profiles appear under **Connections** in the Explorer when no folder is
open. Click a connection to connect, or right-click it to **Edit connection…**,
**Rename…**, **Remove saved connection**, **Move up** or **Move down**. Rename opens
the connection form with the name selected; type a name and click **Save profile**.
The name is the first field and does not change the SSH address. You can also drag a row
above or below another row; the insertion line shows where it will land. Escape
cancels a drag. Changes are saved immediately and the connection picker uses the
same order. Removal affects only RODE's saved profile, not SSH configuration,
keys, known hosts or an existing connection.
Editing a saved profile's name renames it in place; another profile with that
name is preserved rather than overwritten.

Blank optional fields defer to your OpenSSH configuration. Passwords, key passphrases, MFA, and host-key confirmation are handled interactively in the terminal. RODE does not store credentials or disable host-key verification. SSH agent forwarding is governed by your own SSH config. Work channels preserve the authenticated master's agent-forwarding policy; agent forwarding remains off unless you enable it for that host.

To use your desktop's SSH agent for Git operations on a remote host, enable `ForwardAgent yes` for that host in your desktop SSH config, then reconnect RODE and open a new terminal. The desktop process must have access to the agent when it connects. In a normal-mode remote terminal, `ssh-add -l` should list the forwarded identities, and the Git tab uses the same agent. Existing persistent terminals retain their original environment. This is separate from the project's sandbox setting.

An existing alias works directly:

```sh
./target/release/rode --ssh workstation --remote-path '~/projects/simulation'
```

Or configure a direct connection with a jump host:

```sh
./target/release/rode --ssh compute.internal \
  --user researcher --port 22 \
  --identity ~/.ssh/id_ed25519 \
  --jump researcher@login.example.org \
  --remote-path /scratch/researcher/simulation
```

The selected client reads `~/.ssh/config`, including `Include` and `ProxyJump` settings. The connection picker discovers literal aliases in the main config file; aliases defined in included files can be entered manually. Use `--ssh-config FILE` to select an alternative configuration.

The desktop renews its remote gateway and attachment streams every 10 seconds,
including while the window is idle or hidden. If a desktop crashes, reboots or
loses its network, those transient helpers expire after 45 seconds without input,
even if the SSH server still has their pipes open. Their attachments close:
agents in Auto mode finish accepted work and then exit; agents in On mode,
persistent terminals and notebook/debugger runtimes remain available to
reattach. Off/nonpersistent work follows its normal last-view cleanup policy.
Agent and runtime views keep exclusive control while their attachment is live.

## Client compatibility

The Rust backend supports `Host` patterns and negation, bounded `Include` files,
`HostName`, `User`, `Port`, multiple `IdentityFile` entries, `IdentitiesOnly`,
`IdentityAgent`, `ProxyJump`, `HostKeyAlias`, known-hosts files, `BatchMode`, public
key/password/keyboard-interactive authentication, optional Linux Kerberos
authentication, `ForwardAgent`, `SetEnv` and
`SendEnv`. Explicit connection fields take precedence. Jump hosts use native SSH
channels, including multiple hops, without starting another client executable.

Unknown host keys require confirmation unless your configuration explicitly
allows accepting new keys. Changed and revoked keys are always rejected, including
with `StrictHostKeyChecking no`. Hashed known-host entries are supported; new keys
are merged under a lock, with hashed entries by default. Secrets are entered in
the login terminal and are not saved in profiles. User certificates are supported
with a matching `IdentityFile` plus adjacent `-cert.pub`, or through an SSH agent.
Agent authentication and forwarded agent sockets are bound to the SSH session
so supporting agents can enforce destination restrictions.

When a server offers several host-key types, Rust SSH prefers types already
recorded for that host in the configured known-hosts files, including hashed
entries and `HostKeyAlias`. RSA records also match RSA SHA-2 signature algorithms.
The selected key must still match a trusted record exactly; choosing a known type
does not accept a changed key or rewrite existing records.

On Linux, `GSSAPIAuthentication yes` enables Kerberos ticket authentication
(`gssapi-with-mic`) when the server offers it. RODE loads the system MIT/Heimdal
GSSAPI library on demand and uses the existing credentials, including
`KRB5CCNAME` and `KRB5_CONFIG`. The library is optional: missing libraries or
unusable tickets fall back to other enabled authentication methods. Disabled
methods stay disabled; a Kerberos-only login fails with a diagnostic.

Install/configure the Kerberos runtime separately (for example,
`libgssapi-krb5-2` on Debian/Ubuntu), and obtain or renew tickets through your
normal login or `kinit` before connecting. RODE does not run `kinit`, ask for a
Kerberos password, or install libraries. A client `ssh` executable is still
unnecessary. For aliases, set `HostName` to the server name associated with its
Kerberos `host` service principal; hostname canonicalisation and realm selection
follow the system Kerberos configuration, independently of `HostKeyAlias`.

Host keys are verified before Kerberos authentication. Each jump host uses its
own SSH configuration and authenticates from the desktop's credentials.
Credential delegation is off unless `GSSAPIDelegateCredentials yes` applies to
that host; the ticket and server policy must also permit delegation. Delegation
is separate from SSH agent forwarding. GSSAPI-authenticated key exchange is not
implemented; normal SSH key exchange and host-key checks remain in use.

Select **OpenSSH** in the connection form, or pass `--ssh-backend openssh`, for
`Match`, `ProxyCommand`, host certificates/CA policies, GSSAPI key exchange, custom
algorithm policy, arbitrary port forwarding or other unsupported directives.
Unsupported settings fail with an explanation; RODE never silently switches
clients. The Rust backend owns connection timeouts, keepalives, channel sharing
and terminal allocation; OpenSSH process-management options do not control it.
The backend is saved per profile. Older profiles without a backend use Rust.

```sh
rode --ssh workstation --ssh-backend openssh
```

Both clients keep one authenticated connection per window. Closing a project
terminal does not close it. Connection loss ends its channels without attempting
a fresh login or replaying uncertain writes; reconnect explicitly to continue.
Rust control endpoints use the existing sealed process capability on Linux and
reject different mount namespaces, so local sandboxed tools cannot use them to
launch unrestricted remote commands.

## Compute-node return tunnels

Open **Tunnels** beside **Connect SSH**, from **File → SSH tunnels**, or from the
command palette. With a local project or no project open, it manages **This
computer** and needs no initial SSH connection. With an SSH workspace selected,
it manages that remote host; a lost SSH connection never falls back to the local
service. The dialog identifies which host owns the tunnels.

The tunnel host must already run `sshd`. Its session service can maintain an
outbound Rust SSH connection to a relay and request a reverse forward:

```text
RODE desktop → relay SSH → relay 127.0.0.1:22022
                             ↓ reverse SSH connection from compute node
                          compute 127.0.0.1:22 → compute sshd
```

1. Choose **New tunnel**. Enter the relay's literal hostname/IP, SSH port
   and username, a stable relay loopback port, and the compute node's SSH port.
   The outbound worker does not read SSH aliases or ProxyJump configuration.
2. Leave both credential fields blank to try **the compute node's** default SSH
   private-key files: `~/.ssh/id_rsa`, `~/.ssh/id_ecdsa`, then `~/.ssh/id_ed25519`.
   These are the supported file-backed keys in [OpenSSH's default order](https://man.openbsd.org/ssh_config.5#IdentityFile).
   Missing, unreadable, encrypted or rejected defaults do not prevent trying the
   next file. Set **Identity file** to use a specific unencrypted key instead,
   or select a persistent **Agent socket**. With an agent selected and the identity
   blank, only that agent is used. Encrypted and hardware-backed keys require the
   persistent agent; the service does not prompt for passphrases or hardware access.
   RODE does not copy your desktop key to the node or provision credentials.
3. Start the tunnel. Verify the displayed relay SHA-256 host-key fingerprint
   through a trusted source, then choose **Trust relay key and start**. This pin
   belongs to the retained tunnel; it does not edit `known_hosts`. A changed key
   stops retries until explicitly verified and replaced.
4. For a tunnel started through an SSH workspace, once connected, choose
   **Configure return route**. If the desktop reaches the
   relay through a different SSH alias or jump chain, expand **Return connection
   settings** on the **Tunnels** tab and enter **Desktop relay alias / jump chain**
   first. The default is the relay username, hostname and port.
5. In **Connect SSH**, save the generated profile before leaving the direct
   connection. Reconnect with it to reach the same compute host and project
   through the relay. Desktop authentication to both relay and compute node stays
   interactive and uses the normal SSH configuration. The route supports either
   desktop backend; the daemon's outbound tunnel always uses Rust SSH.

When starting locally, credentials and the compute SSH port refer to **this
computer**, even on the welcome screen. No project, local `ssh` executable or
existing service is required. Opening the manager only discovers tunnels;
**Start tunnel** starts the matching local session service if needed.

For a locally started tunnel, use **Copy connection details** once connected
instead of **Configure return route**. On your other computer, open **Connect
SSH**, set Host to `127.0.0.1`, Port to the relay listener port, Jump / ProxyJump
to the supplied relay address (or your desktop relay alias), and User to your
login on the tunnel host. Select that host's credentials and verify its SSH host
key independently of the relay. Save the profile there. These manually entered
profiles use the loopback host/port for known-host lookup; automatic return
profiles from an existing SSH workspace retain its original host-key identity.

The **Tunnels** tab shows each tunnel’s status in a separate card. **Tunnel**
shows the relay hostname and tunnel listener port; **Destination** shows the
compute hostname and SSH port it forwards to. The relay's SSH login port belongs
in setup and connection details, not the card summary. A short note explains that
the tunnel listener is loopback-only and reached through the relay's SSH connection.
The compute name comes from the owning service. Older remote services use the
selected SSH host/alias as a display fallback; local tunnels use this computer's
hostname. These labels do not change the actual loopback forwarding addresses.
**New tunnel** groups relay, forwarding and compute-host credentials;
its draft survives tab switches. Status refreshes pause while setting up a tunnel.
Tabs and the Start/Close controls remain visible while the form scrolls.

Choose **Save configuration** in setup or on a running tunnel's card to keep it
across editor and service restarts. **Saved** lists the reusable setups. Choose
**Load setup** to fill the form, review it, then choose **Start tunnel**. Loading
does not connect or start a service. **Update configuration** saves changes,
including a renamed setup. **Delete…** removes only the saved setup.

Saved setups belong to their tunnel host: local setups include this computer's
hostname; remote setups use the original SSH host/alias, login, port override and
configuration file. Reconnect to that SSH host before loading its setup. Project
folder, SSH backend and return-tunnel routing changes do not change this identity.
The desktop preferences store settings, key/agent paths, the optional return-route
jump chain and any explicitly trusted relay public key. Private keys, passphrases
and agent credentials are never copied into saved setups. Saving from a tunnel
after verifying its relay key retains that pin for subsequent starts.

Automatic return-profile creation currently requires desktop SSH configuration
understood by the Rust backend, even when selecting OpenSSH for the resulting
connection.

The return profile retains the compute host's original configuration alias,
identity settings and known-hosts identity. Only the final network destination
becomes the relay's loopback port. **Remove tunnel route** in Connect SSH restores
ordinary destination routing; edit the jump field as needed. Both SSH hosts are
verified independently; trusting the relay never skips compute-host verification.
Files, tools and terminals then use the normal authenticated workspace connection
and its existing sandbox policy.

The relay must permit remote TCP forwarding for the compute login and direct TCP
forwarding for the desktop login. RODE requests a `127.0.0.1` listener; the relay
administrator must keep `GatewayPorts no` or `clientspecified` to honour that
scope. Other relay users can reach its loopback port, but must still authenticate
to the compute node's SSH server. No session-service control socket is exposed.

The tunnel survives editor exit and closing the last terminal. After a network
loss, the service retries after 1, 2, 4, 8, 16 and then 30 seconds, with SSH
keepalives detecting stalled peers. Existing connections through a failed tunnel
close; reconnect RODE once the tunnel returns. Authentication rejection or a
changed host key requires attention rather than repeated login attempts. Open
**Tunnels** through any working connection to the compute host to inspect, retry
or stop it. If all routes are lost, administrative access to the compute host is
needed to repair credentials or trust.

Running tunnels are retained in service memory, not restarted after a daemon restart,
node reboot or allocation ending. **Stop tunnel** closes its forwarded streams;
it leaves unrelated terminals, jobs and tunnels alone. Starting a tunnel does
not grant persistence to project jobs or extend a scheduler allocation.

Tunnel failures identify the operation and the relevant identity file, agent
socket, relay address or listener port. Paths are resolved on the tunnel host.
Blank credentials try the supported default key files on that host; an explicit
key or agent selection overrides this search. The worker does not read `IdentityFile`
or other settings from SSH configuration and never inherits `SSH_AUTH_SOCK`.
Changing the desktop's key path does not change a remote
tunnel's credentials. Error details include the underlying
OS failure, without displaying private-key contents.

If SSH login succeeds but the relay refuses the tunnel listener, the error
includes a port-specific `ss -ltnp` command to run **on the relay**. A listener
owned by `sshd-session` usually belongs to another SSH tunnel. Stop that tunnel
at its source, or choose a different **Relay port** under **Port forwarding**;
leave the relay's **SSH port** unchanged. If the port is free, have the relay
administrator check `AllowTcpForwarding`, `DisableForwarding`, `PermitListen`
and restrictions on the authorised key. SSH sends no reason for this refusal,
so RODE reports these as possible causes rather than claiming a confirmed port
conflict. Transport failures and request timeouts are reported separately.
The service keeps retrying; stop the RODE tunnel before an `ssh -R` test on the
same listener. **Copy error** copies the full displayed diagnostic for inspection
or sharing, including errors returned by older services. Copying never starts
a command or changes the relay's configuration.

Already running session services keep their existing code after an editor
update. Worker diagnostic improvements take effect after the owning service is
restarted; restarting it ends its tunnels and must account for any retained work.

Unattended relay authentication currently supports plain private keys and
agent-held keys/certificates. Passwords, keyboard-interactive/MFA, Kerberos,
encrypted keys without a persistent agent, and relay host certificates are not
supported by this worker. A forwarded desktop agent can disappear when the
original connection closes and is unsuitable for recovery. These limits do not
restrict the desktop's ordinary SSH authentication. At most eight tunnels and
32 simultaneous forwarding channels per tunnel are retained.

Tunnel management is an explicit host-administration action outside the project
sandbox, through the existing protected session-service channel. Settings travel
in bounded protocol messages, never in process arguments. No local `ssh` binary
is required on either the desktop or the compute node for this workflow.

## X11 applications

**Forward X11 when available** in the connection form requests X11
forwarding (OpenSSH compatibility mode uses `-X`). It is saved with the profile and works through ProxyJump.
The desktop running RODE needs an X server (or XWayland), a valid `DISPLAY` and
`xauth`; the remote SSH server needs `X11Forwarding yes` and `xauth`. If these
are unavailable, the connection and ordinary terminal still work.

Open a new remote terminal and run an X application, such as `xclock`. The SSH server
sets the remote `DISPLAY` and manages authentication cookies; do not copy the
desktop's display value to the remote shell. Local terminals already inherit
the desktop display environment. Native Wayland forwarding is not included.

Clear the checkbox, or use `rode --ssh HOST --no-x11`, to disable forwarding
even if your SSH config enables it. When checked, the profile explicitly
requests forwarding; `ForwardX11Trusted`, `ForwardX11Timeout` and
`XAuthLocation` follow your desktop SSH config. **X11 trust** can explicitly
override `ForwardX11Trusted` for either backend. **SSH configuration** preserves
the selected client's default: Rust uses untrusted forwarding, while some
OpenSSH installations default to trusted forwarding. If X applications work
through your normal trusted SSH connection, choose **Trusted (full desktop
access)** to request the same policy in RODE. This permits remote X applications
to access other desktop windows and input; it must be an explicit choice.
Reconnect SSH and open a new terminal after changing it. The Rust backend accepts
`ForwardX11Timeout` as a number of seconds; zero keeps forwarding available for
the connection's lifetime. A positive value stops accepting new X11 clients
after that many seconds, even though an existing shell still has `DISPLAY` set.
Reconnect SSH to renew an expired display. The Rust backend uses a
connection-specific proxy cookie and requires successful untrusted-cookie
generation when trusted forwarding is disabled; failure disables X11 while
leaving ordinary SSH usable. See
the [OpenSSH X11 settings](https://man.openbsd.org/ssh_config#ForwardX11) for
trust and timeout behaviour.

The native gateway owns the forwarded listener, so detaching and reattaching
a terminal view within the same SSH connection keeps it usable. Disconnecting
SSH closes that display tunnel, even for persistent shells. After reconnecting,
open a **new terminal** for X applications. RODE preserves existing programs
and does not inject replacement display settings into their shells. Already
running X applications can lose their display when SSH disconnects.

Forwarding does not relax the project's sandbox. Network-isolated terminals
cannot reach the SSH display listener. A filesystem sandbox also needs explicit
read access to the remote Xauthority file (usually `~/.Xauthority`, or the file
selected by `XAUTHORITY`). Add that file in **Sandbox access rules** if needed;
RODE does not expose display sockets or authorization files automatically.
Compute-node applications need the site's own Slurm X11 support as well.

## Host-service backends

All SSH connections use the native Rust `rode-sessiond` helper. Files, protected saves, Git, tools, telemetry, mise integration, terminal/process/debugger coordination, runtime-cache deployment and sandbox preparation share Rust services with local execution. Python is required only for Python-specific features such as interpreter inspection, debugpy and the Jupyter kernel adapter. Other installed tools retain their own [host requirements](project-tools.md#formatting-issues-and-python-environments).

RODE first looks for a compatible **preinstalled `rode-sessiond` on the remote host**: in its non-interactive `PATH`, then `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin` and `/usr/bin`. Discovery checks at most five candidates within a bounded deadline. Incompatible or older installations are skipped; if none is compatible, RODE uses the existing verified deployment of a desktop-supplied helper. It never overwrites the installed package or downloads/builds a helper automatically.

To select one installation explicitly, set **Remote helper** in the SSH connection form (saved with that profile), or use:

```sh
rode --ssh workstation --remote-helper '~/.cargo/bin/rode-sessiond'
rode --ssh workstation --remote-helper /opt/rode/bin/rode-sessiond
```

Paths refer to the **remote host**, allow spaces, and must be absolute or start with `~/`. They are passed as literal arguments. An explicit missing, unsafe or incompatible helper fails with an error; it never silently selects another installation or uploads a replacement. With a compatible installed helper, the desktop needs no binary for the remote architecture.

### Compatibility and upgrades

`rode-sessiond --host-info` prints bounded JSON containing its package version, OS/architecture, metadata format, helper API revision, host and session protocol versions, capabilities, executable path, size and SHA-256. The package version identifies a release; compatibility requires matching metadata/API/protocol versions and every required capability. Different package releases can work together when those contracts match. Helpers predating this metadata are not accepted as installed candidates. Update the remote helper, use automatic deployment, or point RODE at another compatible installation.

Before executing a discovered helper, RODE resolves symlinks and checks that the executable and its canonical ancestors are owned by the user or root and not writable by another user (sticky ancestor directories such as `/tmp` are allowed). Setuid/setgid executables are rejected. A compatible helper is **copied on the remote host** into the private immutable cache at `~/.rode-host/host-<sha256>`; no executable bytes are transferred from the desktop. Size and digest are checked before publication, and the live gateway handshake repeats the identity and compatibility checks before any project operation. Existing cached generations are verified and reused. Upgrading the installed package therefore does not replace the executable used by a live connection or persistent job. Reconnect to discover the new installation; keep cached generations while jobs reference them.

### Supported hosts and deployment fallback

Remote hosts still need **Linux x86_64 or aarch64**, a POSIX shell, `uname`, and GNU-compatible `id`, `stat`, `sha256sum`, `wc`, `head`, `mktemp`, `ln`, `chmod`, `cut`, `mkdir`, `rm` and `rmdir`. Installed-helper discovery also uses `readlink -f`. A preinstalled helper does not add support for macOS, BSD or Windows. x86_64 is locally validated; aarch64 selection is supported but does not establish execution coverage there.

The recommended desktop source installation, `cargo xtask install`, builds and installs a static Linux helper beside the editor; Debian packages supply one too. When deployment is needed, this avoids older-glibc startup failures. Plain `cargo install` builds a helper linked against the build host's libc. Build a matching [static helper](terminals.md#remote-helper-and-packaging) for a different architecture or libc; cross-compilation requires a suitable toolchain.

Desktop `RODE_HOST_HELPER` selects a file or a directory containing `<architecture>/rode-sessiond`; it takes precedence over `RODE_SESSION_HELPER`. These explicit desktop overrides bypass automatic remote discovery. A profile/CLI **Remote helper** path takes precedence over both. Without overrides, deployment checks `host-helpers/<architecture>/rode-sessiond` beside the editor, matching musl build output and same-architecture sibling helpers.

Both installed and uploaded helpers use the same verified private cache. Its home/cache must be writable by the owner and executable; no root access, remote compiler, GUI libraries or application port forwarding is needed. A `noexec` home, unsafe paths, missing utilities or incompatible runtime libraries produce explicit errors. Use a static helper for libc differences. A damaged cache entry is preserved and rejected, never silently replaced.

One authenticated stdio channel owns the native gateway and up to four concurrent project operations. Project contexts capture the actual sandbox and cannot be weakened by an ordinary request. Terminal and process streams use separate channels on the same authenticated connection. Disconnect cancels transient work; [persistent sessions](terminals.md) retain their separate owner. Native failure never retries through Python or outside an active sandbox. After a lost write acknowledgement, the write may have completed: reconnect and reconcile the remote revision before trying another save. A damaged helper cache fails verification without overwriting the suspect file.


## Connection and workspace lifetime

Saved profiles live in `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json`. **Save profile** persists connection settings, not passwords. Clicking a saved connection in the explorer connects immediately using that profile; unsaved edits still require a decision before switching. Use **Connect SSH** to open the form for new connections or editing profiles. **Remote: Reconnect SSH** in the command palette restores the last connection. A dedicated SSH client process owns the authenticated connection; project terminals use separate channels. Hiding or detaching a project terminal leaves the connection available. **New** creates an independent shell with persistence off; **Sessions** discovers/reattaches existing ones. Enable **Persist: on** in the terminal header to leave work running when RODE closes without an exit prompt; tmux/screen is not required. See [terminal lifetime and remote helper setup](terminals.md).

The Rust SSH master receives its launch profile through an owner-only file in its private connection directory, which it removes after opening. Its process arguments contain only the internal mode, socket path and a SHA-256 digest that detects substituted profile data before authentication. Helper discovery and bootstrap scripts travel over the existing SSH input stream, followed by any executable upload; `ps` shows a short launcher instead of the full script. Host verification, interactive login and helper digest checks still apply. Restart the editor and reconnect to use these launch changes; existing processes keep their original arguments.

Use **Disconnect** next to the SSH host in the top-left connection banner, or **Remote: Disconnect SSH** in the command palette. Disconnecting, losing the connection, or a failed login clears the selected host and project. The title and explorer return to the local, no-folder state; **Open project folder…** and **Open file…** work locally immediately. Open remote tabs and unsaved edits stay in memory as disconnected buffers. Use **Save As** for a local copy, or **Remote: Reconnect SSH** to reconnect to their original project and restore their file bindings. Their original paths are never used for local file access. Opening a local project follows the usual unsaved-change protection. Disconnect stops project kernels/builds and detaches agent/terminal views: **Persist: on** sessions continue, while **Persist: off** sessions end after their last view closes. Reattach agents through **Agent chat → Running** under their original project policy. Disconnect is briefly unavailable while terminal/agent startup or a persistence change is pending.

If login fails or the connection ends unexpectedly, the terminal shows **SSH ERROR / host** and retains the login's output, including client errors such as `Permission denied` or `Connection refused`. It is read-only: the SSH process is stopped, and this is not a local shell. Scroll to earlier output, use **Copy**, and choose **New** when you want a local shell. Folder/file opening already uses the local computer; **Remote: Reconnect SSH** starts a fresh attempt. The bounded terminal output stays only in this window's memory and is cleared when replaced by a new terminal/project or the window closes. RODE does not save the login output to preferences or a log file.

## Project folders and sandbox mode

[Encrypted projects](encrypted-projects.md) can be unlocked on the connected host through **File → Open encrypted project…**. gocryptfs and FUSE run there; the desktop uses the existing SSH channels. Lock the encrypted project before explicitly disconnecting.

The SSH host and connection state are displayed in a persistent banner, the status bar, and the window title. **File → Open project folder…** / **Ctrl+Shift+O** browses the currently selected host: remote folders when connected over SSH, local folders otherwise. Navigate with the path field, **Browse**, the parent-folder arrow, and the folder list, then choose **Open project** (or **Ctrl+Enter**). The explorer, relative file paths, saves, and project terminal then use that folder. Switching remote projects reuses the authenticated connection and protects unsaved buffers. **File → Open local folder…** explicitly switches back to this machine.

Enable sandboxing and choose **bwrap** or **Landlock** in the folder picker or SSH profile. You can allow/block network access and grant additional read-only or read-write paths. The selected backend is probed on the project host. If it is unavailable, preparation returns a persistent **NORMAL MODE · sandbox unavailable** indicator and the reason; it is not presented as isolated. Once active, an operation failure never triggers an unrestricted retry.

```sh
./target/release/rode /path/to/project --sandbox
./target/release/rode --ssh workstation --remote-path '~/projects/simulation' --sandbox
```

When available, bubblewrap isolates both editor file operations and project terminals. The desktop UI and SSH transport stay outside the sandbox. System/tool files are read-only, the project and selected tool caches are writable, and the rest of the home directory is hidden. Sandbox preparation does not install tools; see the [Ruff runner](project-tools.md#formatting-issues-and-python-environments) for its cache behavior. See [sandbox behavior, shared paths, and limitations](sandbox.md).

**View → Workspace panels → Project settings** opens the access editor. Set each built-in path to **Hidden**, **Read only**, or **Read + write**; add paths with their own access level; control networking and the project's base read-only mode. **Apply sandbox settings** probes the new policy, preserves editor buffers, and opens a shell under the new policy. Existing persistent shells detach and retain their original policy; they are not silently migrated or stopped. Settings are saved locally per host/project. The folder browser recalls a saved folder's policy, and CLI startup restores it; `--sandbox` forces sandboxing on with that project's saved rules.
