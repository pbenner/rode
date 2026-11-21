# Remote connections

RODE uses an embedded Rust SSH client (`russh`) by default. The desktop needs no
`ssh`, `scp` or `ssh-keygen` executable for connections, helper deployment, file
operations or project terminals. The remote machine still needs a running SSH
server and a POSIX shell. External programs such as Git-over-SSH retain their own
client requirements.

Open **Connections** in the left rail, then choose **Connect SSH…**. Every connection can configure:

- Client backend: **Rust (default)** or **OpenSSH**
- SSH config alias, hostname, or `user@hostname`
- Username, port, identity file, and remote folder
- One or more comma-separated ProxyJump hosts
- An alternative SSH config file
- An optional preinstalled remote helper path
- X11 forwarding when available (enabled by default)
- An optional **Connection name**, such as `Work cluster` or `Home server`, shown
  in Connections and the connection picker instead of the SSH address

The left-rail **Connections** button opens a resizable sidebar beside the current
editor, Welcome screen or application tool. Click it again to hide it. The top
provides **Open project…**, **Connect SSH…** and **New tunnel…**; below are saved
projects, SSH connections and saved tunnels in one scrolling list. Right-click a
saved tunnel and choose **Remove saved tunnel**, then confirm deletion. The menu
also works for tunnels on other hosts; running tunnels are unaffected. With the
row focused, **Shift+F10** opens the same menu. Explorer contains the project file
browser, independently of these saved locations.
The same Connections sidebar can detach through its rail context menu.

Click a saved SSH connection to connect, or right-click it to **Edit connection…**,
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

Once a connection finishes opening the workspace, RODE shows Welcome with Explorer
hidden. Choose **Open project folder** to browse folders on the connected SSH host,
or select **Explorer** or **Project tools** in the left rail to start work.
Reconnecting with open buffers returns to the editor and preserves their unsaved
changes. The project shell starts in the background; use **Toggle terminal** when
you need it.

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

## File transfers

Choose **File transfers** in the left rail or **Remote: File transfers** in the
command palette. Select the connected SSH host using **Host**. The remote Explorer also
offers **Download…** and **Upload here…**. The optional **File transfers** extension
must be enabled; disabling it hides these controls and cancels its work.

Right-click **File transfers** in the left rail and choose **Open in separate
window** to keep the browsers beside your project. Return using **Reattach** in
that window, **Reattach to main window** in the icon's context menu, or the native
window close button. Browsed folders, checked files and queued/running copies stay
with the same controller while the view moves.

The view has independent, resizable **Local files** and **Remote files** panes,
with vector file icons, a folder address bar, filtering and a compact activity tray.
Use **List view** or **Icon view** beside each filter to choose compact rows or
larger file tiles independently for the two panes. Switching views keeps checked
files, the navigation cursor and the current folder. Tiles wrap into fewer columns
as the window narrows; long names use two lines and expose their full path on hover.
Type a path and press **Enter** or the arrow beside it; use **Parent folder** to
move up, or double-click a folder. The **···** menu contains Home, Refresh, hidden
files, selection actions and New folder. **Select this folder** copies the current
folder itself, including a sandboxed project root. Browsing does not change the
project, chat, terminal or execution settings. Drag the pane/queue dividers to
resize. Dividers highlight on hover or keyboard focus. Focus a divider with Tab
or action hints, then use Left/Right for the browser split or Up/Down for the
queue height. Press Home or double-click the divider to restore its default size.
On short windows, the tray becomes an **Activity…** bar; open it for progress,
cancel, retry and finished copies while both file panes retain their space.

Use **Rename…** or **Delete…** in a pane's **···** menu for checked files, or
right-click a file or folder. **F2** renames one item and **Delete** opens the
confirmation for checked items (or the keyboard cursor when nothing is checked).
Rename never replaces an existing destination. Delete permanently removes the
selected entries, including folder contents; symbolic-link targets are preserved.
The confirmation names the host and items and initially focuses **Cancel**.
The current folder itself cannot be deleted via **Select this folder**.
File operations are disabled while copies are active and follow the selected
host's access policy.
Project editor paths update after a successful rename; deleting files keeps open
buffers and unsaved edits available for **Save As**. Both browser views refresh
after a file operation.

Drag files or folders to the opposite pane to copy them into that displayed
folder, or drop onto a folder row to copy into that folder. Dragging a selected
row copies the whole selection; dragging an unselected row copies just that item.
A highlighted destination and copy cursor show valid targets. Escape cancels a
drag, and dropping back on the source pane does nothing. A drag is rejected if
its source listing or selected transfer scope changed. You can also drop local files/folders
from your desktop file manager onto the remote pane to upload. Desktop drops
require actual local filesystem paths; virtual attachments are not supported.
Every drop uses the same queue, filesystem policy and collision choices as the
buttons; it always copies and leaves the source intact.

Click rows to check several files/folders, or focus the list and use arrows,
**Space** to check entries, and **Enter** to enter a directory. **Right** also opens
directories in List view; in Icon view, all four arrows move between tiles and
**Backspace** opens the parent folder. Checked files have a prominent filled
checkmark, an accent border and a tinted background in both views;
the keyboard cursor has a separate outline, so clearing selection removes the fill.
**Upload** copies selected local entries into the displayed remote directory;
**Download** copies the other way. Entire directory trees and empty directories
are included. Symbolic links and special files are rejected rather than followed.

The resizable **Activity** tray shows progress and average speed and supports cancellation,
removing waiting jobs and explicitly retrying failures. Existing files require
**Skip**, **Replace** or **Rename**, with an option to apply that decision to the
remaining conflicts in the batch. A blank rename chooses an unused name. Existing
directories are merged; directories are never replaced by files. Copies use saved
disk contents and refuse destinations with unsaved editor changes. New files are
private to your user and retain the source owner's execute bit; replacement files
retain destination permissions.

Transfers reuse the authenticated SSH master and native helper, with dedicated
channels and bounded read/write windows. They require no separate transfer tool,
remote Python or SFTP subsystem. **Access → Use current project permissions** captures
that project's filesystem policy, including read-only restrictions and explicit
grants. It expires when the project changes. **Access → Use host access (outside
project sandbox)** explicitly prepares ordinary user access to the selected SSH
host, initially at its home folder; that scope survives project switches on the
same connection. Opening the view or navigating application areas never widens
permissions. The local browser uses this computer's normal permissions. A denied
operation never retries outside its captured policy. Older helpers need the `streaming_file_transfers` capability.

Each file is staged beside its destination, checked for length/SHA-256 and
published atomically. Cancellation or failure abandons the current stage; files
already completed remain. A disconnect can lose the final acknowledgement, so
check the destination before retrying. Switching projects cancels work using project permissions. Disconnecting the selected
host, locking an encrypted project or disabling the extension cancels queued/running work;
normal window exit waits for local cleanup. Interrupted copies restart through
an explicit retry; automatic synchronization and resume are not implemented.

## Shared folders

Open **Project tools → Shared folders** (or **Project: Shared folders** in the
command palette), then select **Add shared folder**. Choose
**Local folder on SSH host** or **SSH folder on this computer**. Use **Browse…**
beside each path to select a source folder and an empty mount folder on the
receiving machine. The picker shows its host, supports folder filtering, hidden
folders, parent/home navigation and keyboard selection. In the mount-folder
picker, **New folder** creates and opens an empty directory. Cancelling a picker
keeps the form's paths unchanged. Choose **Save shared folder** when ready.
You can still enter paths directly: paths are absolute or start with `~/`, interpreted on their
respective hosts. Mount folders must be owned by you and not writable by other
users. Choose a specific source subfolder; whole-home and filesystem-root exports
are refused.

Select **Mount** to make the source available through the mount path. **Read only**
is enabled by default and enforced by both the mount and exporting server. Turn it
off to allow programs on the receiving machine to edit the source files. Changes
are live after files are saved; this creates no offline copy or synchronization
queue. Applications writing concurrently need their own coordination; shares do
not provide cross-host file locking. Up to eight
shared folders may be configured. Symbolic links and special files are excluded.

The receiving machine needs **SSHFS**, **FUSE**, and `fusermount3` (or
`fusermount`). Linux is currently supported. On Debian/Ubuntu these are provided
by `sshfs` and `fuse3`. Compute hosts may prohibit FUSE even when these tools are
installed; RODE reports the mount failure and preserves the existing folders.
Both the native Rust SSH backend and OpenSSH reuse their authenticated connection
through dedicated binary channels; no additional login, SSH server on the desktop,
network listener or installed SFTP server is required. Remote source access uses
the current project's policy, independently of the File transfers host and access
selection. Mounting does not add project sandbox grants;
sandboxed programs need access to the resulting mount path, and existing isolated
shells may need to be reopened after a mount is created.

**Unmount** refuses an in-use mount. Close programs using it and retry. Cancel
an unfinished attempt with **Cancel mount**. RODE automatically unshares folders
before changing the project, project sandbox policy or SSH connection, disabling
the File transfers extension, or closing RODE. The change waits for cleanup to
finish. If a mount is busy, the
change stops and the current workspace stays open; close programs using the
folder and try the change again.
Hiding or detaching the project tab keeps mounts running. Changing the File
transfers browser's host or access scope leaves project shares unchanged. Lost
connections and abrupt desktop exits tear down their streams and detach owned
mounts; open handles then report I/O failures. After a mounting-helper crash or forced kill, a stale FUSE
mount may need manual `fusermount3 -u /mount/path` cleanup.

New SSH bookmarks include the current shared-folder settings by default. The small
bookmark icon sits to the right of **Saved projects** in Connections. Right-click
it to turn sharing inclusion off for new bookmarks, or right-click a saved
bookmark and toggle **Include shared-folder settings** for that project.
**Update saved shared folders** refreshes its snapshot after changing the recipes.
Reopening a bookmark automatically mounts its saved folders after the project and
SSH connection are ready, using that project's current access policy. Loading the
saved-project list alone never mounts folders. Mount failures are reported in
the Shared folders tab; they do not retry automatically. Existing bookmarks without
shared-folder settings keep their project-only behavior.

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

Use **New tunnel…** at the top of Connections to open the setup form. The
full-width **Active Tunnels** button under **Saved tunnels** opens the separate
active-tunnel list. **File → SSH tunnels** and the command palette also open
that list. With a local project or no project open, it manages **This
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
   and username, a stable relay listener port, and the compute node's SSH port.
   **Relay access** defaults to **Localhost only**. Choose **All network interfaces**
   to request access from other devices; saved setups retain this choice.
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
in setup and connection details, not the card summary. A short note distinguishes
access through the relay's SSH connection from access requested for other devices.
**Listen on** shows the requested listener scope; the relay's policy can restrict it.
The compute name comes from the owning service. Older remote services use the
selected SSH host/alias as a display fallback; local tunnels use this computer's
hostname. These labels do not change the fixed loopback destination on the compute host.
**New tunnel** groups relay, forwarding and compute-host credentials;
its draft survives closing and reopening the form or viewing active tunnels.
Background status checks do not disable setup fields or actions. The Start/Close
controls remain visible while the form scrolls. Starting a tunnel opens the active
list so you can review its status and verify its relay key.

The Connections sidebar’s **Active Tunnels** button shows the connected count, or connected/total
when some retained tunnels are connecting, reconnecting or need attention.
Hover for the breakdown. **Active Tunnels (?)** means status is unknown or the host
is unavailable. Status is checked every 15 seconds outside the list and every
2 seconds while viewing it, on the current host only. These read-only checks
never start a tunnel service or reconnect SSH.

Choose **Save configuration** in setup or on a running tunnel's card to keep it
across editor and service restarts. Saved setups appear in the **Connections**
sidebar, below SSH connections. Click a setup to fill the form, review it, then
choose **Start tunnel**. Loading does not connect or start a service. The tunnel
list and setup form are separate dialogs; saved setups are managed in the
sidebar. **Update configuration** saves changes, including a renamed setup.
Right-click a saved tunnel, choose **Remove saved tunnel**, and confirm **Delete configuration**
to remove only its saved setup; running tunnels remain intact.

Setups for another host are disabled; hover for the host required to load them.
Their remove controls remain available without connecting to that host.

Saved setups belong to their tunnel host: local setups include this computer's
hostname; remote setups use the original SSH host/alias, login, port override and
configuration file. Reconnect to that SSH host before loading its setup. Project
folder, SSH backend and return-tunnel routing changes do not change this identity.
The desktop preferences store settings, key/agent paths, the optional return-route
jump chain and any explicitly trusted relay public key. Private keys, passphrases
and agent credentials are never copied into saved setups. Saving from a tunnel
after verifying its relay key retains that pin for subsequent starts.

To change an existing tunnel, use **Copy settings** to prepare the new setup.
Stop the original before starting the copy on the same relay port; stopping it
closes connections through that tunnel. Saving a configuration edits the reusable
setup and does not change a running listener.

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
forwarding for return connections through SSH. **Localhost only** requests
`127.0.0.1`; **All network interfaces** requests all address families supported by
the relay. Other devices can then connect to the relay hostname and listener port,
using the compute host's SSH login and credentials. **Copy connection details**
includes this direct route for an all-interface tunnel; the SSH return route
remains available too.

To honour the client's choice, the relay administrator should set
[`GatewayPorts clientspecified`](https://man.openbsd.org/sshd_config#GatewayPorts)
in `sshd_config`, validate it with `sshd -t`, then reload that SSH service using
the host's normal administration procedure. `GatewayPorts no` (the server default)
restricts forwarding to loopback even when all interfaces were requested.
`GatewayPorts yes` forces wildcard listeners, including for localhost requests;
use `clientspecified` to preserve each tunnel's selected scope. The relay firewall
must also permit the listener port. SSH confirms the forwarding request without
reporting the effective bind address, so RODE displays the requested scope;
inspect the relay's listeners with `ss -ltnp` to verify its actual binding.

Network listeners expose the compute host's existing SSH server to devices that
can reach the relay port. Those clients must still authenticate to the compute
host and verify its host key independently. No session-service control socket is
exposed. Existing and older saved setups retain the localhost default.

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
administrator check `GatewayPorts`, `AllowTcpForwarding`, `DisableForwarding`, `PermitListen`
and restrictions on the authorised key. SSH sends no reason for this refusal,
so RODE reports these as possible causes rather than claiming a confirmed port
conflict. Transport failures and request timeouts are reported separately.
The service keeps retrying; stop the RODE tunnel before an `ssh -R` test on the
same listener. **Copy error** copies the full displayed diagnostic for inspection
or sharing, including errors returned by older services. Copying never starts
a command or changes the relay's configuration.

Already running session services keep their existing code after an editor
update. Listener-scope changes and worker diagnostic improvements require an
updated owning service; restarting it ends its tunnels and must account for any
retained work. RODE reports when an older service ignores the requested scope.

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

Saved profiles live in `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json`. **Save profile** persists connection settings, not passwords. Clicking a saved connection in Connections connects immediately using that profile; unsaved edits still require a decision before switching. Use **Connect SSH** to open the form for new connections or editing profiles. **Remote: Reconnect SSH** in the command palette restores the last connection. A dedicated SSH client process owns the authenticated connection; project terminals use separate channels. Hiding or detaching a project terminal leaves the connection available. **New** creates an independent shell with persistence off; **Sessions** discovers/reattaches existing ones. Enable **Persist: on** in the terminal header to leave work running when RODE closes without an exit prompt; tmux/screen is not required. See [terminal lifetime and remote helper setup](terminals.md).

The Rust SSH master receives its launch profile through an owner-only file in its private connection directory, which it removes after opening. Its process arguments contain only the internal mode, socket path and a SHA-256 digest that detects substituted profile data before authentication. Helper discovery and bootstrap scripts travel over the existing SSH input stream, followed by any executable upload; `ps` shows a short launcher instead of the full script. Host verification, interactive login and helper digest checks still apply. Restart the editor and reconnect to use these launch changes; existing processes keep their original arguments.

Choose **Disconnect** beside **Open project…** in the top header, open the SSH badge in the bottom status bar and choose **Disconnect SSH**, or use **Remote: Disconnect SSH** in the command palette. The header button appears only while SSH is connected. Disconnecting, losing the connection, or a failed login clears the selected host and project. The title and explorer return to the local, no-folder state; **Open project folder…** and **Open file…** work locally immediately. Open remote tabs and unsaved edits stay in memory as disconnected buffers. Use **Save As** for a local copy, or **Remote: Reconnect SSH** to reconnect to their original project and restore their file bindings. Their original paths are never used for local file access. Opening a local project follows the usual unsaved-change protection. Disconnect stops project kernels/builds and detaches agent/terminal views: **Persist: on** sessions continue, while **Persist: off** sessions end after their last view closes. Reattach agents through **Agent chat → Running** under their original project policy. Disconnect is briefly unavailable while terminal/agent startup or a persistence change is pending.

If login fails or the connection ends unexpectedly, the terminal shows **SSH ERROR / host** and retains the login's output, including client errors such as `Permission denied` or `Connection refused`. It is read-only: the SSH process is stopped, and this is not a local shell. Scroll to earlier output, use **Copy**, and choose **New terminal → Outside sandbox** when you want a local shell. Folder/file opening already uses the local computer; **Remote: Reconnect SSH** starts a fresh attempt. The bounded terminal output stays only in this window's memory and is cleared when replaced by a new terminal/project or the window closes. RODE does not save the login output to preferences or a log file.

## Project folders and sandbox mode

[Encrypted projects](encrypted-projects.md) can be unlocked on the connected host through **File → Open encrypted project…**. gocryptfs and FUSE run there; the desktop uses the existing SSH channels. Lock the encrypted project before explicitly disconnecting.

The bottom status bar separates a **LOCAL** or **SSH · user@host** badge from the project path and access mode. The SSH badge also identifies connecting/offline states; its menu offers **Disconnect SSH** and **Reconnect SSH**. Clicking the project path opens a bounded folder popup with **Copy path** and **Open project…**; long paths scroll independently of those actions. The native window title also identifies the SSH host. **Open project…** in the top header (**File → Open project folder…** / **Ctrl+Shift+O**) browses the currently selected host: remote folders when connected over SSH, local folders otherwise. Navigate with the path field, **Browse**, the parent-folder arrow, and the folder list, then choose **Open project** (or **Ctrl+Enter**). The explorer, relative file paths, saves, and project terminal then use that folder. Switching remote projects reuses the authenticated connection and protects unsaved buffers. **File → Open local folder…** explicitly switches back to this machine.

Enable sandboxing and choose **bwrap** or **Landlock** in the folder picker or SSH profile. You can allow/block network access and grant additional read-only or read-write paths. The selected backend is probed on the project host. If it is unavailable, the bottom status bar shows **Normal · unavailable**; open its access-mode menu or hover it for the full policy and reason. The project is not presented as isolated. Once active, an operation failure never triggers an unrestricted retry.

```sh
./target/release/rode /path/to/project --sandbox
./target/release/rode --ssh workstation --remote-path '~/projects/simulation' --sandbox
```

When available, bubblewrap isolates both editor file operations and project terminals. The desktop UI and SSH transport stay outside the sandbox. System/tool files are read-only, the project and selected tool caches are writable, and the rest of the home directory is hidden. Sandbox preparation does not install tools; see the [Ruff runner](project-tools.md#formatting-issues-and-python-environments) for its cache behavior. See [sandbox behavior, shared paths, and limitations](sandbox.md).

**View → Workspace panels → Project settings** opens the access editor. Set each built-in path to **Hidden**, **Read only**, or **Read + write**; add paths with their own access level; control networking and the project's base read-only mode. **Apply sandbox settings** probes the new policy, preserves editor buffers, and opens a shell under the new policy. Existing persistent shells detach and retain their original policy; they are not silently migrated or stopped. Settings are saved locally per host/project. The folder browser recalls a saved folder's policy, and CLI startup restores it; `--sandbox` forces sandboxing on with that project's saved rules.
