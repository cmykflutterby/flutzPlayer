#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: hydrate-linux-source.sh SOURCE_ROOT CACHE_ROOT" >&2
    exit 2
fi

source_root=$1
cache_root=$2
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
export PATH="$cargo_home/bin:$PATH"
fetch_root="$cache_root/vendor-fetch"
target_root="$cache_root/target"
sdl_destination="$source_root/vendor/SDL3/sdl3-sys-0.6.2+SDL-3.4.4-spacefix"
jemalloc_destination="$source_root/vendor/tikv-jemalloc-sys"
xkb_destination="$source_root/vendor/xkbcommon-dl"
xkb_patch_root="$source_root/build/vendor-patches"

mkdir -p "$fetch_root/src" "$target_root" "$source_root/.cargo" "$source_root/vendor/SDL3"
cat > "$fetch_root/Cargo.toml" <<'EOF'
[package]
name = "flutz_linux_vendor_fetch"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
sdl3-sys = "=0.6.2+SDL-3.4.4"
tikv-jemalloc-sys = "=0.6.1"
xkbcommon-dl = "=0.4.2"
EOF
printf 'fn main() {}\n' > "$fetch_root/src/main.rs"

cargo fetch --manifest-path "$fetch_root/Cargo.toml" --locked 2>/dev/null ||
    cargo fetch --manifest-path "$fetch_root/Cargo.toml"

find_registry_crate() {
    local package_prefix=$1
    local match
    match=$(find "$cargo_home/registry/src" -mindepth 2 -maxdepth 2 -type d \
        -name "${package_prefix}*" -print -quit)
    [[ -n "$match" ]] || {
        echo "Cargo fetched ${package_prefix}, but its registry source directory was not found." >&2
        exit 1
    }
    printf '%s\n' "$match"
}

hydrate_crate() {
    local package_prefix=$1
    local destination=$2
    local source
    source=$(find_registry_crate "$package_prefix")
    if [[ -f "$destination/Cargo.toml" ]] &&
        [[ -f "$destination/build.rs" ]] &&
        cmp -s "$source/Cargo.toml" "$destination/Cargo.toml"; then
        return
    fi
    rm -rf -- "$destination"
    mkdir -p "$(dirname "$destination")"
    cp -a -- "$source" "$destination"
}

hydrate_crate "sdl3-sys-0.6.2+SDL-3.4.4" "$sdl_destination"
hydrate_crate "tikv-jemalloc-sys-0.6.1" "$jemalloc_destination"

# xkbcommon-dl: crates.io source + tracked patch + checksum-pinned libxkbcommon source.
# shellcheck source=../../vendor-patches/xkbcommon-dl.env
source "$xkb_patch_root/xkbcommon-dl.env"
hydrate_xkbcommon_dl() {
    local patch_file="$xkb_patch_root/$XKBCOMMON_DL_PATCH"
    local stamp_file="$xkb_destination/.flutz-hydrated"
    local stamp tarball source
    stamp="xkbcommon-dl=$XKBCOMMON_DL_VERSION patch=$(sha256sum "$patch_file" | cut -d' ' -f1) libxkbcommon=$LIBXKBCOMMON_SHA256"
    if [[ -f "$stamp_file" ]] && [[ "$(cat "$stamp_file")" == "$stamp" ]]; then
        return
    fi
    tarball="$fetch_root/libxkbcommon-$LIBXKBCOMMON_VERSION.tar.gz"
    if ! [[ -f "$tarball" ]] || ! echo "$LIBXKBCOMMON_SHA256  $tarball" | sha256sum --check --status; then
        curl --fail --location --silent --show-error --output "$tarball.partial" "$LIBXKBCOMMON_URL"
        echo "$LIBXKBCOMMON_SHA256  $tarball.partial" | sha256sum --check --status || {
            echo "libxkbcommon $LIBXKBCOMMON_VERSION download failed checksum verification." >&2
            rm -f -- "$tarball.partial"
            exit 1
        }
        mv -- "$tarball.partial" "$tarball"
    fi
    source=$(find_registry_crate "xkbcommon-dl-$XKBCOMMON_DL_VERSION")
    rm -rf -- "$xkb_destination"
    cp -a -- "$source" "$xkb_destination"
    patch --batch --silent --forward -p1 -d "$xkb_destination" < "$patch_file"
    mkdir -p "$xkb_destination/libxkbcommon"
    tar -xzf "$tarball" --strip-components=1 -C "$xkb_destination/libxkbcommon"
    printf '%s\n' "$stamp" > "$stamp_file"
}
hydrate_xkbcommon_dl

cat > "$source_root/.cargo/config.toml" <<EOF
[build]
target-dir = "$target_root"
EOF

test -f "$sdl_destination/Cargo.toml"
test -f "$jemalloc_destination/Cargo.toml"
test -f "$xkb_destination/build.rs"
test -f "$xkb_destination/libxkbcommon/meson.build"
grep -Fq "target-dir = \"$target_root\"" "$source_root/.cargo/config.toml"
echo "LINUX_VENDOR_OK sdl3-sys=0.6.2+SDL-3.4.4 tikv-jemalloc-sys=0.6.1 xkbcommon-dl=$XKBCOMMON_DL_VERSION+libxkbcommon-$LIBXKBCOMMON_VERSION target=$target_root"
