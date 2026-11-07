#!/bin/sh
set -eu
case "${1:-remove}" in
  remove|purge)
    profile=/etc/apparmor.d/app.freedombooks.FreedomBooks
    if [ -f "$profile" ]; then
      if command -v apparmor_parser >/dev/null 2>&1; then
        apparmor_parser -R "$profile" || echo 'FreedomBooks: could not unload AppArmor profile' >&2
      fi
      rm -f "$profile"
    fi
    ;;
esac
if [ "$(readlink /usr/bin/freedombooks 2>/dev/null || true)" = /opt/FreedomBooks/freedombooks ]; then
  rm -f /usr/bin/freedombooks
fi
update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
