#!/usr/bin/env bash
set -euo pipefail

manifest_path=/var/lib/flutzplayer-build/provisioning-manifest.json
guest_source_path=/home/vagrant/flutzPlayer-src
guest_artifact_path=/home/vagrant/flutzPlayer-artifacts
rdp_source_cidr=
provisioning_fingerprint=
check_only=false

while (($#)); do
    case "$1" in
        --manifest) manifest_path="${2:?missing manifest path}"; shift 2 ;;
        --source) guest_source_path="${2:?missing source path}"; shift 2 ;;
        --artifacts) guest_artifact_path="${2:?missing artifact path}"; shift 2 ;;
        --rdp-cidr) rdp_source_cidr="${2:?missing RDP CIDR}"; shift 2 ;;
        --fingerprint) provisioning_fingerprint="${2:?missing provisioning fingerprint}"; shift 2 ;;
        --check-only) check_only=true; shift ;;
        *) echo "Unknown verification argument: $1" >&2; exit 2 ;;
    esac
done

fail() {
    echo "VERIFY_FAILED: $*" >&2
    exit 1
}

. /etc/os-release
[[ "$ID" == ubuntu ]] || fail "guest OS is not Ubuntu"
[[ "$VERSION_ID" == 26.04 ]] || fail "guest release is not Ubuntu 26.04"
grep -Fq '26.04.1' /var/log/installer/media-info ||
    fail "guest point release is not Ubuntu 26.04.1"
[[ "$(uname -m)" == x86_64 ]] || fail "guest architecture is not x86_64"

for service in ssh gdm3 gnome-remote-desktop.service; do
    systemctl is-enabled --quiet "$service" || fail "$service is not enabled"
    systemctl is-active --quiet "$service" || fail "$service is not active"
done
systemctl is-enabled --quiet gnome-remote-desktop-configuration.service ||
    fail "gnome-remote-desktop-configuration.service is not enabled"
if systemctl is-active --quiet xrdp 2>/dev/null; then
    fail "xrdp must not compete with GNOME Remote Desktop for TCP 3389"
fi

for directory in \
    "$guest_source_path" \
    "$guest_artifact_path" \
    /home/vagrant/flutzPlayer-cache \
    /home/vagrant/flutzPlayer-staging \
    /home/vagrant/flutzPlayer-logs; do
    [[ -d "$directory" && -w "$directory" ]] ||
        fail "guest build directory is unavailable or not writable: $directory"
done

disk_bytes="$(lsblk -b -d -n -o SIZE | sort -nr | head -n1)"
[[ "$disk_bytes" -ge 85899345920 ]] || fail "guest disk is smaller than 80 GiB"

rdp_test_user="$(</etc/flutzplayer-build/rdp-test-user)"
id "$rdp_test_user" >/dev/null 2>&1 || fail "dedicated RDP test user is missing"
[[ "$(id -u "$rdp_test_user")" -ne 0 ]] || fail "RDP test user must not be root"
rdp_test_groups="$(id -nG "$rdp_test_user")"
[[ " $rdp_test_groups " == *" sudo "* ]] || fail "RDP test user must have sudo access"
sudoers_file="/etc/sudoers.d/99-$rdp_test_user"
[[ -f "$sudoers_file" ]] || fail "RDP test user sudoers policy is missing"
visudo -cf "$sudoers_file" >/dev/null 2>&1 || fail "RDP test user sudoers policy is invalid"
grep -Fqx "$rdp_test_user ALL=(ALL:ALL) NOPASSWD: ALL" "$sudoers_file" ||
    fail "RDP test user does not have full passwordless sudo access"
[[ "$(runuser --user "$rdp_test_user" -- sudo -n id -u)" == 0 ]] ||
    fail "RDP test user cannot exercise passwordless root sudo access"

for legacy_user in flutz-tester; do
    [[ "$legacy_user" == "$rdp_test_user" ]] && continue
    ! id "$legacy_user" >/dev/null 2>&1 ||
        fail "legacy RDP test account still exists: $legacy_user"
done

if [[ -z "$rdp_source_cidr" ]]; then
    rdp_source_cidr="$(</etc/flutzplayer-build/rdp-source-cidr)"
fi
lan_address="$(python3 - "$rdp_source_cidr" <<'PY'
import ipaddress
import subprocess
import sys
network = ipaddress.ip_network(sys.argv[1], strict=True)
output = subprocess.check_output(["ip", "-o", "-4", "addr", "show", "scope", "global"], text=True)
matches = []
for line in output.splitlines():
    fields = line.split()
    address = ipaddress.ip_interface(fields[3]).ip
    if address in network and str(address) not in matches:
        matches.append(str(address))
if not matches:
    raise SystemExit(f"Expected at least one LAN address in {network}; found none")
route = subprocess.check_output(["ip", "-4", "route", "get", "1.1.1.1"], text=True).split()
if "src" in route:
    preferred = route[route.index("src") + 1]
    if preferred in matches:
        print(preferred)
        raise SystemExit(0)
print(matches[0])
PY
)" || fail "bridged LAN address detection failed"

gateway="$(ip -4 route show default | awk 'NR == 1 {print $3}')"
[[ -n "$gateway" ]] || fail "default gateway was not found"
ping -c 1 -W 5 "$gateway" >/dev/null || fail "default gateway is unreachable"
curl --fail --location --silent --show-error --range 0-0 \
    https://static.rust-lang.org/ >/dev/null || fail "Rust build endpoint is unreachable"
curl --fail --location --silent --show-error --range 0-0 \
    https://github.com/ >/dev/null || fail "GitHub build endpoint is unreachable"

ss -lntp | awk '$4 ~ /:3389$/ && $0 ~ /gnome-remote-de/ {found=1} END {exit !found}' ||
    fail "GNOME Remote Desktop is not listening on TCP 3389"
[[ "$(grep -E '^WaylandEnable=' /etc/gdm3/custom.conf | tail -n1)" == "WaylandEnable=true" ]] ||
    fail "GDM is not configured for the required Wayland session"
runtime_dir="$(mktemp -d /run/flutzplayer-grdctl-verify.XXXXXX)"
chown gnome-remote-desktop:gnome-remote-desktop "$runtime_dir"
chmod 0700 "$runtime_dir"
grd_status="$(runuser --user gnome-remote-desktop -- \
    env HOME=/var/lib/gnome-remote-desktop SHELL=/bin/bash XDG_RUNTIME_DIR="$runtime_dir" \
    dbus-run-session -- grdctl --system status)"
rm -rf "$runtime_dir"
grep -Fq 'Status: enabled' <<<"$grd_status" ||
    fail "GNOME Remote Desktop RDP backend is not enabled"
grep -Fq 'Port: 3389' <<<"$grd_status" ||
    fail "GNOME Remote Desktop is not configured for TCP 3389"
grep -Fq 'Authentication methods: credentials' <<<"$grd_status" ||
    fail "GNOME Remote Desktop credential authentication is not configured"
grep -Fq 'TLS certificate: /etc/gnome-remote-desktop/rdp-tls.crt' <<<"$grd_status" ||
    fail "GNOME Remote Desktop TLS certificate is not configured"
grep -Fq 'TLS key: /etc/gnome-remote-desktop/rdp-tls.key' <<<"$grd_status" ||
    fail "GNOME Remote Desktop TLS key is not configured"
ufw_status="$(ufw status verbose)"
grep -Eq '^Default: deny \(incoming\), allow \(outgoing\)' <<<"$ufw_status" ||
    fail "guest firewall does not default-deny incoming traffic"
grep -Fq "$rdp_source_cidr" <<<"$ufw_status" ||
    fail "trusted-CIDR firewall rules are missing"
grep -Eq "^22/tcp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+$rdp_source_cidr" <<<"$ufw_status" ||
    fail "development SSH TCP 22 trusted-CIDR firewall rule is missing"
grep -Eq "^3389/tcp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+$rdp_source_cidr" <<<"$ufw_status" ||
    fail "GNOME Remote Desktop TCP 3389 trusted-CIDR firewall rule is missing"

required_commands=(
    gcc g++ clang cmake ninja meson bison patch make pkg-config autoconf automake libtool nm
    git curl rsync ssh sha256sum desktop-file-validate update-desktop-database
    mksquashfs fusermount3 tar xz jq aplay
)
for command_name in "${required_commands[@]}"; do
    command -v "$command_name" >/dev/null || fail "$command_name is unavailable"
done
pkg-config --exists xcb-xkb || fail "xcb-xkb development files are unavailable"
[[ -f "$(pkg-config --variable=libdir xcb-xkb)/libxcb-xkb.a" ]] ||
    fail "static libxcb-xkb.a is unavailable"

export RUSTUP_HOME=/home/vagrant/.rustup
export CARGO_HOME=/home/vagrant/.cargo
export PATH="$CARGO_HOME/bin:$PATH"
command -v rustc >/dev/null || fail "rustc is unavailable"
command -v cargo >/dev/null || fail "cargo is unavailable"
command -v rustfmt >/dev/null || fail "rustfmt is unavailable"
command -v cargo-clippy >/dev/null || fail "cargo-clippy is unavailable"
rust_toolchain="$(jq -er '.rust.toolchain' /etc/flutzplayer-build-tool-versions.json)"
[[ "$(rustc --version)" == "rustc $rust_toolchain "* ]] ||
    fail "rustc does not match pinned toolchain $rust_toolchain"

probe_root="$(mktemp -d)"
trap 'rm -rf "$probe_root"' EXIT
cat >"$probe_root/probe.c" <<'EOF'
int main(void) { return 0; }
EOF
gcc -Werror "$probe_root/probe.c" -o "$probe_root/c-probe"
"$probe_root/c-probe"

cat >"$probe_root/probe.rs" <<'EOF'
fn main() { println!("rust-probe-ok"); }
EOF
rustc "$probe_root/probe.rs" -o "$probe_root/rust-probe"
"$probe_root/rust-probe" | grep -Fqx rust-probe-ok

cat >"$probe_root/sdl-probe.c" <<'EOF'
#include <SDL3/SDL.h>
int main(void) { return SDL_GetVersion() > 0 ? 0 : 1; }
EOF
gcc -Werror "$probe_root/sdl-probe.c" -o "$probe_root/sdl-probe" \
    $(pkg-config --cflags --libs sdl3)
"$probe_root/sdl-probe"

for module in x11 wayland-client alsa libpulse libpipewire-0.3; do
    pkg-config --exists "$module" || fail "pkg-config module $module is unavailable"
done

[[ -r /proc/asound/cards ]] || fail "guest ALSA sound core is unavailable"
grep -Eq 'ICH|AC97|Intel82801' /proc/asound/cards ||
    fail "guest AC97 audio hardware is not present; enable VirtualBox audio with the AC97 controller"
aplay -l 2>/dev/null | grep -Eq '^card [0-9]+:' ||
    fail "no ALSA playback device is available for testers"
for audio_user in vagrant "$rdp_test_user"; do
    [[ " $(id -nG "$audio_user") " == *" audio "* ]] ||
        fail "$audio_user is not in the audio group and cannot reach the playback device"
    runuser --user "$audio_user" -- aplay -l 2>/dev/null | grep -Eq '^card [0-9]+:' ||
        fail "$audio_user cannot enumerate an ALSA playback device"
done

cat >"$probe_root/flutzplayer.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=flutzPlayer Probe
Exec=/bin/true
Icon=utilities-terminal
Categories=AudioVideo;
EOF
desktop-file-validate "$probe_root/flutzplayer.desktop"

if dpkg-query -W -f='${db:Status-Status}\n' libfuse2 2>/dev/null | grep -Fqx installed ||
   dpkg-query -W -f='${db:Status-Status}\n' libfuse2t64 2>/dev/null | grep -Fqx installed; then
    fail "libfuse2 is installed; Phase 4 requires FUSE3 without a libfuse2 dependency"
fi

linuxdeploy_asset="$(jq -er '.linuxdeploy.asset' /etc/flutzplayer-build-tool-versions.json)"
appimagetool_asset="$(jq -er '.appimagetool.asset' /etc/flutzplayer-build-tool-versions.json)"
for asset in "$linuxdeploy_asset" "$appimagetool_asset"; do
    expected="$(jq -er --arg asset "$asset" '
        .linuxdeploy as $linuxdeploy |
        .appimagetool as $appimagetool |
        if $linuxdeploy.asset == $asset then $linuxdeploy.sha256
        elif $appimagetool.asset == $asset then $appimagetool.sha256
        else error("unknown asset") end
    ' /etc/flutzplayer-build-tool-versions.json)"
    actual="$(sha256sum "/opt/flutzplayer-tools/$asset" | awk '{print $1}')"
    [[ "$actual" == "$expected" ]] || fail "$asset checksum does not match its pin"
    APPIMAGE_EXTRACT_AND_RUN=1 "/opt/flutzplayer-tools/$asset" --version >/dev/null
done

validate_manifest() {
    python3 - "$1" /etc/flutzplayer-provisioning-manifest.schema.json <<'PY'
import json
import pathlib
import sys

import jsonschema

instance = json.loads(pathlib.Path(sys.argv[1]).read_text())
schema = json.loads(pathlib.Path(sys.argv[2]).read_text())
validator_class = jsonschema.validators.validator_for(schema)
validator_class.check_schema(schema)
validator_class(schema, format_checker=jsonschema.FormatChecker()).validate(instance)
PY
}

if [[ "$check_only" == true ]]; then
    [[ -f "$manifest_path" ]] || fail "provisioning manifest is missing"
    validate_manifest "$manifest_path" || fail "provisioning manifest does not match its schema"
    [[ "$provisioning_fingerprint" =~ ^[a-f0-9]{64}$ ]] ||
        fail "expected provisioning fingerprint is invalid"
    [[ -f /var/lib/flutzplayer-build/provisioning-inputs.sha256 ]] ||
        fail "provisioning fingerprint is missing"
    [[ "$(</var/lib/flutzplayer-build/provisioning-inputs.sha256)" == "$provisioning_fingerprint" ]] ||
        fail "guest provisioning inputs are stale"
else
    install -d -m 0755 "$(dirname "$manifest_path")"
    temporary_manifest="${manifest_path}.tmp"
    jq -n \
        --arg rustup_toolchain "$(rustup show active-toolchain | awk '{print $1}')" \
        --arg rustc_version "$(rustc --version)" \
        --arg cargo_version "$(cargo --version)" \
        --arg gnome "$(gnome-shell --version)" \
        --arg gnome_remote_desktop "$(dpkg-query -W -f='${Version}' gnome-remote-desktop)" \
        --arg linuxdeploy_release "$(jq -r '.linuxdeploy.release' /etc/flutzplayer-build-tool-versions.json)" \
        --arg linuxdeploy_asset "$linuxdeploy_asset" \
        --arg linuxdeploy_sha "$(jq -r '.linuxdeploy.sha256' /etc/flutzplayer-build-tool-versions.json)" \
        --arg appimagetool_release "$(jq -r '.appimagetool.release' /etc/flutzplayer-build-tool-versions.json)" \
        --arg appimagetool_asset "$appimagetool_asset" \
        --arg appimagetool_sha "$(jq -r '.appimagetool.sha256' /etc/flutzplayer-build-tool-versions.json)" \
        --arg lan_address "$lan_address" \
        --arg rdp_source_cidr "$rdp_source_cidr" \
        --arg provisioned_at "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" \
        '{
            schemaId: "https://flutzplayer.local/schemas/linux-provisioning-manifest/v1",
            ubuntu: {release: "26.04.1", architecture: "x86_64"},
            rust: {
                rustupToolchain: $rustup_toolchain,
                rustcVersion: $rustc_version,
                cargoVersion: $cargo_version
            },
            requiredChecks: [
                {name: "c-compiler", status: "working", identity: "gcc"},
                {name: "rust-compiler", status: "working", identity: $rustc_version},
                {name: "sdl3", status: "working", identity: "pkg-config:sdl3"},
                {name: "bridged-lan", status: "working", identity: $lan_address},
                {name: "rdp-firewall", status: "working", identity: $rdp_source_cidr}
            ],
            desktop: {
                gnome: $gnome,
                gnomeRemoteDesktop: $gnome_remote_desktop,
                remoteDesktopServiceEnabled: true,
                remoteDesktopServiceActive: true,
                sessionType: "wayland"
            },
            appImageTools: {
                linuxdeploy: {
                    release: $linuxdeploy_release,
                    asset: $linuxdeploy_asset,
                    sha256: $linuxdeploy_sha
                },
                appimagetool: {
                    release: $appimagetool_release,
                    asset: $appimagetool_asset,
                    sha256: $appimagetool_sha
                }
            },
            provisionedAtUtc: $provisioned_at
        }' >"$temporary_manifest"
    validate_manifest "$temporary_manifest" || fail "generated provisioning manifest does not match its schema"
    chmod 0644 "$temporary_manifest"
    mv -f "$temporary_manifest" "$manifest_path"
fi

printf 'ENVIRONMENT_OK hostname=%s lan=%s release=26.04.1 arch=x86_64\n' \
    "$(hostname)" "$lan_address"
