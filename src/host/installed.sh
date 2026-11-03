# List installed-helper candidates or probe one literal path without starting a service.
set -eu
candidate=$1
if [ "$candidate" = --list ]; then
    printf 'RODE-HELPERS\000'
    found=$(command -v rode-sessiond 2>/dev/null || true)
    for path in "$found" "$HOME/.cargo/bin/rode-sessiond" "$HOME/.local/bin/rode-sessiond" /usr/local/bin/rode-sessiond /usr/bin/rode-sessiond; do
        case "$path" in /*) ;; *) continue ;; esac
        if [ -f "$path" ] && [ -x "$path" ]; then printf '%s\000' "$path"; fi
    done
    exit 0
fi
case "$candidate" in
    '~/'*) candidate="$HOME/${candidate#\~/}" ;;
    /*) ;;
    *) echo 'Remote helper must be an absolute path or start with ~/' >&2; exit 1 ;;
esac
for program in readlink id stat; do
    command -v "$program" >/dev/null || { echo "Installed helper discovery requires $program" >&2; exit 1; }
done
candidate=$(readlink -f -- "$candidate")
[ -f "$candidate" ] && [ -x "$candidate" ] || { echo 'Installed native helper is missing or not executable' >&2; exit 1; }
uid=$(id -u)
path=$candidate
while :; do
    owner=$(stat -c %u -- "$path")
    mode=$(stat -c %a -- "$path")
    if [ "$path" = "$candidate" ]; then
        [ $((0$mode & 06000)) -eq 0 ] || { echo 'Installed native helper must not be setuid or setgid' >&2; exit 1; }
    fi
    [ "$owner" = "$uid" ] || [ "$owner" = 0 ] || { echo 'Installed native helper path has an unsafe owner' >&2; exit 1; }
    if [ $((0$mode & 022)) -ne 0 ]; then
        # Sticky ancestors such as /tmp cannot replace an owned child directory.
        [ "$path" != "$candidate" ] && [ $((0$mode & 01000)) -ne 0 ] || {
            echo 'Installed native helper or parent is writable by another user' >&2; exit 1;
        }
    fi
    [ "$path" != / ] || break
    path=${path%/*}
    [ -n "$path" ] || path=/
done
exec "$candidate" --host-info
