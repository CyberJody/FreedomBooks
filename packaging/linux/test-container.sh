#!/bin/bash
set -euo pipefail
apt-get update -qq
# Install the product before test-only tools so missing runtime deps cannot be hidden.
apt install --no-install-recommends -y "$FB_DEB"
ldd /opt/FreedomBooks/freedombooks > /results/ldd-runtime.txt
ldd /opt/FreedomBooks/resources/app.asar.unpacked/native/fb-node.node > /results/ldd-native.txt
if grep -q 'not found' /results/ldd-*.txt; then exit 1; fi
test -L /usr/bin/freedombooks
test -f /usr/share/applications/app.freedombooks.FreedomBooks.desktop
test "$(stat -c '%a' /opt/FreedomBooks/chrome-sandbox)" = 755
apt install --no-install-recommends -y nodejs xvfb xauth xdotool dbus-x11 fonts-dejavu-core
useradd --create-home --uid 1000 fbpackaging
install -d -m 700 -o fbpackaging -g fbpackaging /home/fbpackaging/runtime /home/fbpackaging/tmp
chown -R fbpackaging:fbpackaging /results
runuser -u fbpackaging -- env HOME=/home/fbpackaging XDG_CONFIG_HOME=/home/fbpackaging/.config XDG_CACHE_HOME=/home/fbpackaging/.cache XDG_DATA_HOME=/home/fbpackaging/.local/share XDG_RUNTIME_DIR=/home/fbpackaging/runtime TMPDIR=/home/fbpackaging/tmp dbus-run-session -- xvfb-run -a -s '-screen 0 1600x1100x24' node /src/packaging/smoke-installed.cjs /usr/bin/freedombooks /results DEB
# Uninstall leaves the user's encrypted books intact and removes the launcher.
apt remove -y freedombooks
test ! -e /usr/bin/freedombooks
test -f /home/fbpackaging/.local/share/FreedomBooks/books/default.fbbook
echo 'DEB INSTALL/UNINSTALL OK: apt dependencies resolved; sandbox retained; user book retained'
