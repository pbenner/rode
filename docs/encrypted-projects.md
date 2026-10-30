# Encrypted projects

RODE uses an installed **gocryptfs** on the project host to present an encrypted folder as a normal editable project. Backing files and names stay encrypted as writes occur; opening unlocks a FUSE view rather than copying the project into a plaintext directory. No crypto algorithm or disk format is implemented by RODE.

## Open or create

1. Choose **File → Open encrypted project…**, or **Project: Open encrypted project** in the command palette.
2. Select **On the connected SSH host** for remote storage. Authenticate SSH first; the existing connection carries passwords and file operations.
3. Enter the encrypted storage folder as an absolute path or `~/` path, plus its password, then **Unlock & open**. The gocryptfs executable and unmount tool paths are configurable.
4. To create storage, enable **Create new encrypted storage** and confirm the new password. The selected folder must be empty, or have an existing parent if RODE needs to create it. Existing plaintext files are never converted, deleted or replaced.

Cancelling an unlock lets the host operation finish and performs normal cleanup; normal exit waits for cleanup to finish. A refused cleanup must be resolved with **Lock existing mount**.

After opening, the explorer, Git, terminals, agents, language tools, notebooks and previews use the decrypted project root. **ENCRYPTED PROJECT · UNLOCKED** stays visible. The optional sandbox is prepared around this root; mount management itself runs on the host outside that sandbox. The backing store is not added as a project grant.

Existing gocryptfs forward-mode storage using its standard `gocryptfs.conf` is supported. A blank password can reuse a mount already unlocked by RODE under the same host account. RODE verifies the mount's filesystem type and unique identity before accepting it; an ordinary directory or a tool merely exiting successfully is insufficient. There is no plaintext fallback when encryption cannot start. Encryption remains active when optional bubblewrap falls back to normal mode.

## Lock and exit

Use **Encryption / Lock… → Stop project tools & lock**, or **Project: Lock encrypted project**. Save or explicitly discard edits first. Stop terminal jobs with **Sessions → End**, including detached/shared persistent jobs, and persistent agents with **Agent chat → Running → End**. Locking stops RODE's nonpersistent agent, notebook kernels, builds and adapters, waits for file operations, and requests a normal unmount. It never force-unmounts or lazily detaches a filesystem.

Another RODE window, a running terminal/job, or a sandbox still referencing the decrypted filesystem prevents locking. A refused lock remains clearly **UNLOCKED**, keeps buffers available, and reports what must be stopped. Same-account mount namespaces are checked because unmounting one host path alone need not remove a sandbox's bind mount. Host restrictions that prevent this check can also prevent locking.

A successful lock closes the project's tabs and releases its previews. The last RODE window must lock successfully before normal exit; it cannot silently background an encrypted project with persistent jobs. Closing a view while another RODE window still holds the project releases only that view, leaving the other window responsible for locking. Lock before switching to another project/host or explicitly disconnecting SSH. Opening subfolders or changing the sandbox within the same encrypted mount retains the lease. Saved SSH profiles remain available.

Crashes, forced termination and network loss cannot guarantee locking: the host helper attempts an ordinary unmount, and leaves a busy filesystem serving its jobs rather than disrupting them. Its status must be checked on that host. Reconnect SSH, then explicitly unlock again to restore access; remote unsaved buffers remain available. Use **Lock existing mount** in the encrypted-project dialog to retry locking storage that remained open after an interruption. Outside RODE, `fusermount3 -u MOUNTPOINT` (or `fusermount -u`) performs a normal unmount after users of it stop.

## Requirements and storage

Linux, gocryptfs and working FUSE access (`/dev/fuse` plus the host's mount permissions) are required where the data lives. Locking needs `fusermount3` or `fusermount`. A remote host needs the existing Python 3 helper support. RODE installs nothing and does not elevate privileges. A host without FUSE cannot use this feature; it must expose a user-managed decrypted filesystem through an independently configured workflow instead.

RODE-managed plaintext mounts live under `/tmp/rode-vaults-UID/HASH/plain`, in owner-only directories. Their contents come from FUSE, not a disk copy. RODE coordinates its managed mount on one host; independent mounts or copies on other hosts are not tracked. Stable owner-only lock files coordinate windows; do not remove these runtime directories or locks while mounts are in use. The underlying empty mountpoint is inaccessible when unmounted, preventing late saves from writing plaintext into it. Passwords are masked, sent only over stdin, cleared after submission/cancel, and never stored in preferences, environment variables, command arguments or password files. Tool launch failures and password errors do not replace the current workspace.

This protects files stored inside the encrypted root. Preferences, paths, explicitly exported files elsewhere, external tool caches/history, memory and OS swap have their own storage policies. RODE-controlled Marp preview artifacts use RAM-backed scratch when available, or the encrypted root; PDF/image processing stays in memory. Configure external tools and sandbox grants according to your storage needs. Keep backups of encrypted storage including `gocryptfs.conf`; password/key recovery is managed by gocryptfs.

See the upstream [gocryptfs manual](https://github.com/rfjakob/gocryptfs/blob/master/Documentation/MANPAGE.md) and [stable CLI protocol](https://github.com/rfjakob/gocryptfs/blob/master/Documentation/CLI_ABI.md).
