# Encrypted projects

RODE uses an installed **gocryptfs** on the project host to present an encrypted folder as a normal editable project. Backing files and names stay encrypted as writes occur; opening unlocks a FUSE view rather than copying the project into a plaintext directory. No crypto algorithm or disk format is implemented by RODE.

## Open or create

1. Choose **File → Open project folder…** and enable **Encrypted storage (gocryptfs)**. **Open encrypted project…** in the menu or command palette opens the same dialog with encryption already selected.
2. Choose **This computer** or **SSH host** for the storage location. Authenticate SSH first for remote storage; the existing connection carries passwords and file operations.
3. Select the encrypted storage folder in the shared browser, or enter an absolute or `~/` path and use **Browse**. Choose the sandbox backend, presets and access rules in the same form, enter the password, then **Unlock & open**. The selected sandbox is prepared around the decrypted root only after the mount is verified. Executable overrides are under **Encryption tools and recovery**; the usual gocryptfs and automatic unmount defaults need no configuration.
4. To create storage, enable **Create new encrypted storage** and confirm the new password. The selected folder must be empty, or have an existing parent if RODE needs to create it. Existing plaintext files are never converted, deleted or replaced.

You can also right-click a folder in the Explorer and choose **Open as encrypted project…**. This selects the folder on the Explorer's host and opens the form; it does not create storage or unlock anything automatically. Browsing folders retains the credential and access drafts. Closing the project dialog, disabling encryption or changing hosts clears the password. The browser supports parent navigation, filtering, hidden folders and keyboard navigation.

Cancelling an unlock lets the host operation finish and performs normal cleanup; normal exit waits for cleanup to finish. A refused cleanup must be resolved with **Encryption tools and recovery → Lock existing mount**, or the existing-mount management controls.

After opening, the explorer, Git, terminals, agents, language tools, notebooks and previews use the decrypted project root. **Encrypted · unlocked** appears in the bottom status bar beside the connection and project path; narrow windows show **Unlocked**. Click it to open encryption and lock controls, or hover for the storage host and encrypted/decrypted paths. During locking it shows **Locking…**; an unreachable retained lease shows **Unknown**, never a verified unlocked or locked state. The optional sandbox is prepared around the decrypted root; mount management itself runs on the host outside that sandbox. The backing store is not added as a project grant.

Existing gocryptfs forward-mode storage using its standard `gocryptfs.conf` is supported. A blank password can reuse a mount already unlocked by RODE under the same host account. RODE verifies the mount's filesystem type and unique identity before accepting it; an ordinary directory or a tool merely exiting successfully is insufficient. There is no plaintext fallback when encryption cannot start. Encryption remains active if the optional project sandbox is unavailable and the workspace uses normal mode.

## Lock and exit

Use **Close & lock** in the top header, **File → Close project**, **Project: Close project** in the command palette, or click the encrypted-project status in the footer and choose **Stop project tools & lock**. Save or explicitly discard edits first. Stop terminal jobs with **Sessions → End**, including detached/shared persistent jobs, and persistent agents with **Agent chat → Running → End**. Locking stops RODE's nonpersistent agent, notebook kernels, builds and adapters, waits for file operations, and requests a normal unmount. It never force-unmounts or lazily detaches a filesystem.

Another RODE window, a running terminal/job, or a sandbox still referencing the decrypted filesystem prevents locking. A refused lock remains clearly **UNLOCKED**, keeps buffers available, and reports what must be stopped. RODE checks same-account processes' mount tables because unmounting one host path alone need not remove a sandbox's bind mount. Kernel mount IDs distinguish the host mount from bind mounts and namespace copies, including copies at the same path. Access to the protected `/proc/PID/ns/mnt` symlinks is unnecessary. A host that also denies mount-table access can still prevent verification; the error identifies the process that could not be checked.

A successful lock closes the project's tabs and releases its previews, returning to Welcome. SSH remains connected when closing only the project. **Disconnect** runs the same guarded close and releases SSH only after verified unmount; a refused lock keeps the project and connection available. The last RODE window must lock successfully before normal exit; it cannot silently background an encrypted project with persistent jobs. Closing a view while another RODE window still holds the project releases only that view, leaving the other window responsible for locking. Lock before switching to another project/host. Opening subfolders or changing the sandbox within the same encrypted mount retains the lease. Saved SSH profiles remain available.

Crashes, forced termination and network loss cannot guarantee locking: the host helper attempts an ordinary unmount, and leaves a busy filesystem serving its jobs rather than disrupting them. Its status must be checked on that host. Reconnect SSH, then explicitly unlock again to restore access; remote unsaved buffers remain available. Use **Lock existing mount** in the encryption controls to retry locking storage that remained open after an interruption. Outside RODE, `fusermount3 -u MOUNTPOINT` (or `fusermount -u`) performs a normal unmount after users of it stop.

## Requirements and storage

Linux, gocryptfs and working FUSE access (`/dev/fuse` plus the host's mount permissions) are required where the data lives. Locking needs `fusermount3` or `fusermount`. A remote host uses RODE's matching native helper; encrypted-mount management itself does not require Python. RODE does not install filesystem tools or elevate privileges. A host without FUSE cannot use this feature; it must expose a user-managed decrypted filesystem through an independently configured workflow instead.

RODE-managed mounts use the existing [private temporary root](sandbox.md#private-temporary-storage)
on the storage host: `~/.cache/rode/tmp/vaults-HOST_HASH/VAULT_HASH/plain`.
The host hash separates cluster nodes that share a home directory; the vault hash
identifies the canonical encrypted-storage path. The temporary root and
coordination directories are owner-only. Unsafe directories or an unavailable
home/hostname cause an error; RODE does not fall back to public `/tmp`.
Their contents come from FUSE, not a plaintext disk copy.

An explicit host-side `RODE_VAULT_DIR` replaces the `vaults-HOST_HASH` coordination
root. It must be absolute, owned by this user, owner-only and not a symlink.
Every window and helper on that host must use the same value while mounts are in
use. Lock open encrypted projects before changing this location or updating an
installation that uses a different mount location. Existing mounts are never
moved or silently adopted from another coordination root. SSH helpers must
advertise `private_vault_tmp` and `vault_mount_table_scan`; update the native
helper together with the editor. Replacing an executable does not update an
already running editor or encrypted-mount coordinator. Restart with the rebuilt
editor/helper after closing the old mount. A mount opened by the former version
can remain at `/tmp/rode-vaults-UID/VAULT_HASH/plain` until normally unmounted;
reopening it with the current version uses the private location.

Stable lock files coordinate windows; do not remove runtime directories or locks
while mounts are in use. Independent mounts or copies on other hosts are not
tracked. The underlying empty mountpoint is inaccessible when unmounted,
preventing late saves from writing plaintext into it. Passwords are masked,
sent only over stdin, cleared after submission/cancel, and never stored in
preferences, environment variables, command arguments or password files.
Tool launch failures and password errors do not replace the current workspace.

This protects files stored inside the encrypted root. Preferences, paths, explicitly exported files elsewhere, external tool caches/history, memory and OS swap have their own storage policies. RODE-controlled Marp preview artifacts use RAM-backed scratch when available, or the encrypted root; PDF/image processing stays in memory. Configure external tools and sandbox grants according to your storage needs. Keep backups of encrypted storage including `gocryptfs.conf`; password/key recovery is managed by gocryptfs.

See the upstream [gocryptfs manual](https://github.com/rfjakob/gocryptfs/blob/master/Documentation/MANPAGE.md) and [stable CLI protocol](https://github.com/rfjakob/gocryptfs/blob/master/Documentation/CLI_ABI.md).
