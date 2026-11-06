# Fixed rootless bootstrap; validated digest/size and optional installed source.
set -eu
umask 077
for program in id stat sha256sum wc head mktemp ln chmod cut mkdir rm rmdir; do
    command -v "$program" >/dev/null || { echo "Native host bootstrap requires $program" >&2; exit 1; }
done
uid=$(id -u)
check_directory() {
    [ ! -L "$1" ] && [ -d "$1" ] || { echo 'Unsafe native helper directory' >&2; exit 1; }
    [ "$(stat -c %u -- "$1")" = "$uid" ] || { echo 'Native helper directory has a different owner' >&2; exit 1; }
    mode=$(stat -c %a -- "$1")
    [ $((0$mode & 022)) -eq 0 ] || { echo 'Native helper directory is writable by another user' >&2; exit 1; }
}
case "$HOME" in /*) ;; *) echo 'Native helper requires an absolute HOME' >&2; exit 1;; esac
# Resolve an automounted/symlinked home once; reject writable ancestors in the helper subtree.
home=$(cd -- "$HOME" && pwd -P)
check_directory "$home"
# The cache's private mode cannot protect its name if another user can rename
# an ancestor. Root/user-owned sticky parents retain ordinary /tmp fixtures.
ancestor=$home
while :; do
    owner=$(stat -c %u -- "$ancestor")
    mode=$(stat -c %a -- "$ancestor")
    [ "$owner" = "$uid" ] || [ "$owner" = 0 ] || { echo 'Native helper ancestor has an unsafe owner' >&2; exit 1; }
    [ $((0$mode & 022)) -eq 0 ] || [ $((0$mode & 01000)) -ne 0 ] || { echo 'Native helper ancestor is writable by another user' >&2; exit 1; }
    [ "$ancestor" != / ] || break
    ancestor=${ancestor%/*}
    [ -n "$ancestor" ] || ancestor=/
done
directory="$home/.rode-host"
if [ ! -e "$directory" ]; then mkdir -m 700 -- "$directory" 2>/dev/null || [ -d "$directory" ]; fi
check_directory "$directory"
[ "$(stat -c %a -- "$directory")" = 700 ] || { echo 'Native helper cache must be owner-only' >&2; exit 1; }
destination="$directory/host-$1"
verify() {
    [ ! -L "$destination" ] && [ -f "$destination" ] || return 1
    [ "$(stat -c %u -- "$destination")" = "$uid" ] || return 1
    [ "$(stat -c %a -- "$destination")" = 500 ] || return 1
    [ "$(wc -c < "$destination")" -eq "$2" ] || return 1
    [ "$(sha256sum < "$destination" | cut -d ' ' -f 1)" = "$1" ]
}
if [ -e "$destination" ] || [ -L "$destination" ]; then
    verify "$1" "$2" || { echo 'Cached native helper failed verification; existing file was preserved' >&2; exit 1; }
else
    temporary=$(mktemp -d "$directory/.upload.XXXXXXXX")
    trap 'rm -f -- "$temporary/helper"; rmdir -- "$temporary"' EXIT
    trap 'exit 1' HUP INT TERM
    if [ -n "${3:-}" ]; then
        # Snapshot a preinstalled helper on this host, never overwrite its package.
        [ ! -L "$3" ] && [ -f "$3" ] || { echo 'Installed native helper changed before publication' >&2; exit 1; }
        owner=$(stat -c %u -- "$3")
        [ "$owner" = "$uid" ] || [ "$owner" = 0 ] || { echo 'Installed native helper has an unsafe owner' >&2; exit 1; }
        mode=$(stat -c %a -- "$3")
        [ $((0$mode & 022)) -eq 0 ] || { echo 'Installed native helper is writable by another user' >&2; exit 1; }
        [ "$(wc -c < "$3")" -eq "$2" ] || { echo 'Installed native helper size changed; reconnect to discover it again' >&2; exit 1; }
        head -c "$2" -- "$3" > "$temporary/helper"
    else
        head -c "$2" > "$temporary/helper"
    fi
    [ "$(wc -c < "$temporary/helper")" -eq "$2" ] || { echo 'Incomplete native helper payload' >&2; exit 1; }
    [ "$(sha256sum < "$temporary/helper" | cut -d ' ' -f 1)" = "$1" ] || { echo 'Native helper payload failed verification' >&2; exit 1; }
    chmod 500 -- "$temporary/helper"
    # Publication never replaces another generation or a concurrent install.
    ln -- "$temporary/helper" "$destination" 2>/dev/null || verify "$1" "$2"
fi
verify "$1" "$2" || { echo 'Native helper publication failed verification' >&2; exit 1; }
printf '%s\n' "$destination"
