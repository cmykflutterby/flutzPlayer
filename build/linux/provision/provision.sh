#!/usr/bin/env bash
set -euo pipefail

guest_source_path="${1:?guest source path is required}"
guest_artifact_path="${2:?guest artifact path is required}"
guest_manifest_path="${3:?guest manifest path is required}"
rdp_source_cidr="${4:?RDP source CIDR is required}"
provisioning_fingerprint="${5:?provisioning fingerprint is required}"
tool_versions=/tmp/flutzplayer-tool-versions.json
credential_file=/tmp/flutzplayer-rdp-credentials.json

if [[ "$(id -u)" -ne 0 ]]; then
    echo "provision.sh must run as root." >&2
    exit 1
fi

. /etc/os-release
if [[ "$ID" != ubuntu ]] || [[ "$VERSION_ID" != 26.04 ]] ||
   ! grep -Fq '26.04.1' /var/log/installer/media-info ||
   [[ "$(uname -m)" != x86_64 ]]; then
    echo "Provisioning requires Ubuntu 26.04.1 x86_64." >&2
    exit 1
fi

export DEBIAN_FRONTEND=noninteractive
echo "Updating Ubuntu package metadata..."
apt-get update
echo "Installing GNOME Remote Desktop, compiler, desktop, and AppImage prerequisites..."
apt-get install -y \
    ubuntu-desktop gdm3 gnome-remote-desktop dbus-x11 gnome-session gnome-keyring \
    polkitd pkexec network-manager ufw iproute2 iputils-ping python3-jsonschema \
    build-essential clang lld cmake ninja-build meson bison patch make pkg-config \
    autoconf automake libtool libtool-bin git curl ca-certificates wget \
    rsync openssh-client openssh-server jq file patchelf \
    zip unzip tar xz-utils zstd gzip bzip2 coreutils \
    desktop-file-utils shared-mime-info squashfs-tools fuse3 \
    libsdl3-dev \
    libasound2-dev libpulse-dev libpipewire-0.3-dev libjack-jackd2-dev \
    alsa-utils pipewire pipewire-pulse pipewire-audio wireplumber \
    libdbus-1-dev libudev-dev \
    libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxfixes-dev \
    libxi-dev libxss-dev libxtst-dev libxkbcommon-dev \
    libwayland-dev libdecor-0-dev \
    libdrm-dev libgbm-dev libgl1-mesa-dev libegl-dev \
    libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-xkb-dev \
    libgtk-3-dev libssl-dev

install -d -o vagrant -g vagrant -m 0755 \
    "$guest_source_path" \
    "$guest_artifact_path" \
    /home/vagrant/flutzPlayer-cache \
    /home/vagrant/flutzPlayer-staging \
    /home/vagrant/flutzPlayer-logs
install -d -m 0755 /opt/flutzplayer-tools /var/lib/flutzplayer-build

usermod -aG audio vagrant

download_and_verify() {
    local url="$1"
    local sha256="$2"
    local destination="$3"
    local temporary="${destination}.download"
    if [[ -f "$destination" ]] &&
       [[ "$(sha256sum "$destination" | awk '{print $1}')" == "$sha256" ]]; then
        return
    fi
    rm -f "$temporary"
    curl --fail --location --silent --show-error --output "$temporary" "$url"
    printf '%s  %s\n' "$sha256" "$temporary" | sha256sum --check --status
    chmod 0755 "$temporary"
    mv -f "$temporary" "$destination"
}

rustup_url="$(jq -er '.rustup.url' "$tool_versions")"
rustup_sha256="$(jq -er '.rustup.sha256' "$tool_versions")"
rustup_asset="$(jq -er '.rustup.asset' "$tool_versions")"
rust_toolchain="$(jq -er '.rust.toolchain' "$tool_versions")"
rust_profile="$(jq -er '.rust.profile' "$tool_versions")"
download_and_verify "$rustup_url" "$rustup_sha256" "/opt/flutzplayer-tools/$rustup_asset"
chown vagrant:vagrant "/opt/flutzplayer-tools/$rustup_asset"
sudo -u vagrant env HOME=/home/vagrant \
    "/opt/flutzplayer-tools/$rustup_asset" \
    -y --no-modify-path --profile "$rust_profile" --default-toolchain "$rust_toolchain"
sudo -u vagrant env \
    HOME=/home/vagrant \
    RUSTUP_HOME=/home/vagrant/.rustup \
    CARGO_HOME=/home/vagrant/.cargo \
    /home/vagrant/.cargo/bin/rustup component add --toolchain "$rust_toolchain" rustfmt clippy
if ! grep -Fq '.cargo/env' /home/vagrant/.profile; then
    printf '\n[ -f \"$HOME/.cargo/env\" ] && . \"$HOME/.cargo/env\"\n' >>/home/vagrant/.profile
fi
chown vagrant:vagrant /home/vagrant/.profile

for tool in linuxdeploy appimagetool; do
    url="$(jq -er ".${tool}.url" "$tool_versions")"
    sha256="$(jq -er ".${tool}.sha256" "$tool_versions")"
    asset="$(jq -er ".${tool}.asset" "$tool_versions")"
    download_and_verify "$url" "$sha256" "/opt/flutzplayer-tools/$asset"
done

sed 's/\r$//' /tmp/flutzplayer-configure-gnome-remote-desktop.sh \
    >/usr/local/sbin/flutzplayer-configure-gnome-remote-desktop
sed 's/\r$//' /tmp/flutzplayer-verify-environment.sh \
    >/usr/local/sbin/flutzplayer-verify-environment
chmod 0755 \
    /usr/local/sbin/flutzplayer-configure-gnome-remote-desktop \
    /usr/local/sbin/flutzplayer-verify-environment
install -m 0644 "$tool_versions" /etc/flutzplayer-build-tool-versions.json
install -m 0644 /tmp/flutzplayer-provisioning-manifest.schema.json \
    /etc/flutzplayer-provisioning-manifest.schema.json

/usr/local/sbin/flutzplayer-configure-gnome-remote-desktop \
    "$credential_file" \
    "$rdp_source_cidr"

/usr/local/sbin/flutzplayer-verify-environment \
    --manifest "$guest_manifest_path" \
    --source "$guest_source_path" \
    --artifacts "$guest_artifact_path" \
    --rdp-cidr "$rdp_source_cidr" \
    --fingerprint "$provisioning_fingerprint"
printf '%s\n' "$provisioning_fingerprint" \
    >/var/lib/flutzplayer-build/provisioning-inputs.sha256
chmod 0644 /var/lib/flutzplayer-build/provisioning-inputs.sha256

apt-get clean
rm -rf /var/lib/apt/lists/*
echo "Phase 4 guest provisioning completed successfully."
