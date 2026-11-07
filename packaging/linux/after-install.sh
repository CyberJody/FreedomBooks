#!/bin/sh
set -eu
# Prefer Chromium's unprivileged namespace sandbox. The AppArmor profile grants
# userns only to this binary, without changing the host's global restrictions.
profile=/etc/apparmor.d/app.freedombooks.FreedomBooks
if [ -f /opt/FreedomBooks/chrome-sandbox ]; then
  chown root:root /opt/FreedomBooks/chrome-sandbox
  chmod 0755 /opt/FreedomBooks/chrome-sandbox
  # Read procfs directly: minimal installs need not have the sysctl CLI.
  userns=1
  restricted=0
  if [ -r /proc/sys/kernel/unprivileged_userns_clone ]; then
    userns=$(cat /proc/sys/kernel/unprivileged_userns_clone)
  fi
  if [ -r /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]; then
    restricted=$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns)
  fi
  reason=
  if [ "$userns" = 0 ]; then
    reason='kernel.unprivileged_userns_clone=0 disables the namespace sandbox'
  elif [ "$restricted" != 0 ]; then
    mkdir -p /etc/apparmor.d
    printf '%s\n' \
      '# FreedomBooks namespace sandbox (Chromium Ubuntu 24.04 profile pattern).' \
      'abi <abi/4.0>,' \
      'include <tunables/global>' \
      'profile app.freedombooks.FreedomBooks /opt/FreedomBooks/freedombooks flags=(unconfined) {' \
      '  userns,' \
      '  include if exists <local/app.freedombooks.FreedomBooks>' \
      '}' > "$profile"
    chmod 0644 "$profile"
    if command -v apparmor_parser >/dev/null 2>&1 && apparmor_parser -r "$profile"; then
      echo 'FreedomBooks: AppArmor userns profile loaded; chrome-sandbox remains 0755'
    else
      reason='AppArmor restricts user namespaces and its app-specific profile could not be loaded'
    fi
  else
    echo 'FreedomBooks: unprivileged user namespaces available; chrome-sandbox remains 0755'
  fi
  if [ -n "$reason" ]; then
    chmod 4755 /opt/FreedomBooks/chrome-sandbox
    echo "FreedomBooks: using setuid chrome-sandbox (4755): $reason" >&2
  fi
fi
ln -sf /opt/FreedomBooks/freedombooks /usr/bin/freedombooks
update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
