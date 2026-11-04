#!/usr/bin/env bash
set -euo pipefail

credential_file="${1:?credential file is required}"
rdp_source_cidr="${2:?RDP source CIDR is required}"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "configure-gnome-remote-desktop.sh must run as root." >&2
    exit 1
fi
if [[ ! -f "$credential_file" ]]; then
    echo "RDP credential file was not uploaded." >&2
    exit 1
fi

username="$(jq -er '.username' "$credential_file")"
password="$(jq -er '.password' "$credential_file")"
if [[ ! "$username" =~ ^[a-z_][a-z0-9_-]{2,31}$ ]] || [[ "$username" == "root" ]] || [[ "$username" == "vagrant" ]]; then
    echo "RDP username is invalid or reserved." >&2
    exit 1
fi
if (( ${#password} < 12 )) ||
   [[ ! "$password" =~ [[:lower:]] ]] ||
   [[ ! "$password" =~ [[:upper:]] ]] ||
   [[ ! "$password" =~ [[:digit:]] ]] ||
   [[ ! "$password" =~ [^[:alnum:]] ]]; then
    echo "RDP password must be at least 12 characters with upper, lower, numeric, and symbol characters." >&2
    exit 1
fi
if ! python3 - "$rdp_source_cidr" <<'PY'
import ipaddress
import sys
network = ipaddress.ip_network(sys.argv[1], strict=True)
if network.version != 4:
    raise SystemExit(1)
PY
then
    echo "RDP source must be a canonical IPv4 CIDR." >&2
    exit 1
fi

# Remove the account used before the tester was renamed to flutzplayer-tester.
# Guarded to a known legacy name so an existing VM converges without manual cleanup.
legacy_rdp_users=("flutz-tester")
for legacy_user in "${legacy_rdp_users[@]}"; do
    if [[ "$legacy_user" == "$username" ]] || [[ "$legacy_user" == "root" ]] || [[ "$legacy_user" == "vagrant" ]]; then
        continue
    fi
    if id "$legacy_user" >/dev/null 2>&1; then
        echo "Removing legacy RDP test account '$legacy_user'."
        loginctl terminate-user "$legacy_user" >/dev/null 2>&1 || true
        pkill -KILL -u "$legacy_user" >/dev/null 2>&1 || true
        sleep 1
        userdel --remove --force "$legacy_user" >/dev/null 2>&1 || true
    fi
    rm -f "/etc/sudoers.d/99-$legacy_user"
    if getent group "$legacy_user" >/dev/null 2>&1; then
        groupdel "$legacy_user" >/dev/null 2>&1 || true
    fi
    rm -rf "/home/$legacy_user"
done

if ! id "$username" >/dev/null 2>&1; then
    useradd --create-home --shell /bin/bash "$username"
fi
usermod -aG sudo "$username"
usermod -aG audio "$username"
printf '%s:%s\n' "$username" "$password" | chpasswd
passwd --unlock "$username" >/dev/null
printf '%s ALL=(ALL:ALL) NOPASSWD: ALL\n' "$username" >"/etc/sudoers.d/99-$username"
chmod 0440 "/etc/sudoers.d/99-$username"
visudo -cf "/etc/sudoers.d/99-$username" >/dev/null
install -d -m 0755 /etc/flutzplayer-build
printf '%s\n' "$username" >/etc/flutzplayer-build/rdp-test-user
printf '%s\n' "$rdp_source_cidr" >/etc/flutzplayer-build/rdp-source-cidr
chmod 0644 /etc/flutzplayer-build/rdp-test-user /etc/flutzplayer-build/rdp-source-cidr

rm -f "/home/$username/.xsession" /etc/skel/.xsession \
    /etc/polkit-1/rules.d/45-flutzplayer-xrdp.rules

if [[ -f /etc/gdm3/custom.conf ]]; then
    if grep -Eq '^#?WaylandEnable=' /etc/gdm3/custom.conf; then
        sed -i -E 's/^#?WaylandEnable=.*/WaylandEnable=true/' /etc/gdm3/custom.conf
    else
        sed -i '/^\[daemon\]/a WaylandEnable=true' /etc/gdm3/custom.conf
    fi
fi

certificate_dir=/etc/gnome-remote-desktop
certificate_path="$certificate_dir/rdp-tls.crt"
private_key_path="$certificate_dir/rdp-tls.key"
install -d -o gnome-remote-desktop -g gnome-remote-desktop -m 0700 "$certificate_dir"
if [[ ! -s "$certificate_path" || ! -s "$private_key_path" ]]; then
    openssl req -new -newkey rsa:3072 -days 825 -nodes -x509 \
        -subj "/CN=$(hostname --fqdn)" \
        -keyout "$private_key_path" \
        -out "$certificate_path" >/dev/null 2>&1
fi
chown gnome-remote-desktop:gnome-remote-desktop "$certificate_path" "$private_key_path"
chmod 0644 "$certificate_path"
chmod 0600 "$private_key_path"

runtime_dir=/run/flutzplayer-grdctl
install -d -o gnome-remote-desktop -g gnome-remote-desktop -m 0700 "$runtime_dir"
grdctl_system() {
    runuser --user gnome-remote-desktop -- \
        env HOME=/var/lib/gnome-remote-desktop SHELL=/bin/bash XDG_RUNTIME_DIR="$runtime_dir" \
        dbus-run-session -- grdctl --system "$@"
}
grdctl_system rdp set-port 3389
grdctl_system rdp set-auth-methods credentials
grdctl_system rdp set-tls-cert "$certificate_path"
grdctl_system rdp set-tls-key "$private_key_path"
grdctl_system rdp set-credentials "$username" "$password"
grdctl_system rdp disable-view-only
grdctl_system rdp disable-port-negotiation
grdctl_system rdp enable
rm -rf "$runtime_dir"

ufw --force reset >/dev/null
ufw default deny incoming >/dev/null
ufw default allow outgoing >/dev/null
ufw allow from 10.0.2.0/24 to any port 22 proto tcp comment 'Vagrant NAT SSH' >/dev/null
ufw allow from "$rdp_source_cidr" to any port 22 proto tcp comment 'Trusted LAN development SSH' >/dev/null
ufw allow from "$rdp_source_cidr" to any port 3389 proto tcp comment 'Trusted LAN GNOME RDP' >/dev/null
ufw --force enable >/dev/null

systemctl set-default graphical.target
systemctl disable --now xrdp xrdp-sesman 2>/dev/null || true
systemctl enable gdm3 gnome-remote-desktop-configuration.service gnome-remote-desktop.service
systemctl restart gdm3
systemctl restart gnome-remote-desktop-configuration.service gnome-remote-desktop.service
rm -f "$credential_file"
unset password
