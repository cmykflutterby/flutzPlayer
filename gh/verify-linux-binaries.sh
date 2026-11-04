#!/usr/bin/env bash
# CI equivalent of the guest checks in build/linux/LinuxBuild.ps1 and LinuxDat.ps1.
set -euo pipefail

fail() {
    printf 'Linux binary verification error: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 2 ]] || {
    echo 'Usage: verify-linux-binaries.sh APP_BINARY DAT_PACKER' >&2
    exit 2
}
app=$1
packer=$2
[[ -f $app && -x $app ]] || fail "missing or non-executable app binary: $app"
[[ -f $packer && -x $packer ]] || fail "missing or non-executable DAT packer: $packer"

ldd_output=$(ldd "$app" 2>&1) || [[ $ldd_output == *'not a dynamic executable'* ]] ||
    fail "cannot inspect app dependencies: $ldd_output"
[[ $ldd_output != *'not found'* ]] || fail "app has missing libraries: $ldd_output"
[[ $ldd_output != *'libSDL3.so'* ]] || fail "unexpected external SDL3 runtime link: $ldd_output"
[[ $ldd_output != *'libxkbcommon'* ]] || fail "unexpected external xkbcommon runtime link: $ldd_output"
nm --defined-only --format=posix "$app" | awk '
    BEGIN {
        split("SDL_Init xkb_context_new xkb_compose_table_new_from_locale xkb_x11_setup_xkb_extension", required)
    }
    $2 ~ /^[Tt]$/ { defined[$1] = 1 }
    END {
        for (entry in required) {
            if (!(required[entry] in defined)) {
                printf "app binary is missing static function: %s\n", required[entry] > "/dev/stderr"
                failed = 1
            }
        }
        exit failed
    }
' || fail 'app binary is not statically linked against SDL3 and xkbcommon'

packer_ldd=$(ldd "$packer" 2>&1) || [[ $packer_ldd == *'not a dynamic executable'* ]] ||
    fail "cannot inspect DAT packer dependencies: $packer_ldd"
[[ $packer_ldd != *'not found'* ]] || fail "DAT packer has missing libraries: $packer_ldd"
"$packer" --help >/dev/null || fail 'DAT packer did not run'

printf 'Verified %s (%s) and %s (%s)\n' \
    "$app" "$(sha256sum < "$app" | cut -d ' ' -f 1)" \
    "$packer" "$(sha256sum < "$packer" | cut -d ' ' -f 1)"
