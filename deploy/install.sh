#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(id -u) == 0 ]] || { echo 'Run this installer as root.' >&2; exit 1; }
version="${1:?Usage: deploy/install.sh RELEASE_NAME}"
[[ "$version" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || { echo 'Invalid release name' >&2; exit 1; }
test -x target/release/sentinel-offers
id sentinel >/dev/null
root=/opt/sentinel-offers
release="$root/releases/$version"
[[ ! -e "$release" ]] || { echo 'Release already exists.' >&2; exit 1; }
install -d -m 0755 "$root" "$release/bin"
install -d -m 0700 -o sentinel -g sentinel /var/lib/sentinel-offers
install -m 0755 target/release/sentinel-offers "$release/bin/sentinel-offers"
install -m 0755 deploy/start "$root/start"
if [[ ! -f "$root/.env" ]]; then
  install -m 0600 -o sentinel -g sentinel deploy/offers.env.example "$root/.env"
fi
if [[ -e "$root/current" && ! -L "$root/current" ]]; then echo 'current must be a symlink' >&2; exit 1; fi
ln -sfn "$release" "$root/current"
install -m 0644 deploy/sentinel-offers.service /etc/systemd/system/sentinel-offers.service
systemctl daemon-reload
printf 'Installed %s. Start with: systemctl enable --now sentinel-offers\n' "$release"
