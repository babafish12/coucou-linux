#!/usr/bin/env bash
# Build and install for the current user. No root access or system files needed.
set -euo pipefail
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
skip_build=0
autostart=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    --autostart) autostart=1 ;;
    *) echo "Usage: $0 [--skip-build] [--autostart]" >&2; exit 2 ;;
  esac
done
[[ "$(uname -s)" == Linux ]] || { echo 'This installer requires Linux.' >&2; exit 1; }
if (( ! skip_build )); then
  for tool in node npm cargo pkg-config python3; do
    command -v "$tool" >/dev/null || { echo "Missing dependency: $tool" >&2; exit 1; }
  done
  pkg-config --exists gtk+-3.0 webkit2gtk-4.1 dbus-1 || {
    echo 'Missing GTK3, WebKit2GTK 4.1 or D-Bus development files. See docs/LINUX.md.' >&2
    exit 1
  }
  (cd "$repo_dir/windows" && npm ci && npm run tauri -- build --no-bundle)
fi
binary="$repo_dir/windows/target/release/coucou"
[[ -x "$binary" ]] || { echo "Build missing: $binary" >&2; exit 1; }
python3 - "$repo_dir" "$autostart" <<'PY'
import datetime
import json
import os
from pathlib import Path
import shlex
import shutil
import sys

repo = Path(sys.argv[1])
home = Path.home()
data = Path(os.environ.get('XDG_DATA_HOME') or home / '.local/share')
config = Path(os.environ.get('XDG_CONFIG_HOME') or home / '.config')
install_dir = home / '.local/lib/coucou'
launcher = home / '.local/bin/coucou'
icon = data / 'icons/hicolor/128x128/apps/coucou.png'
desktop = data / 'applications/Coucou.desktop'

stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
def write_file(path, content, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and path.read_bytes() != content:
        shutil.copy2(path, path.with_name(path.name + '.backup-' + stamp))
    temporary = path.with_name(path.name + '.tmp-' + stamp)
    temporary.write_bytes(content)
    temporary.chmod(mode)
    temporary.replace(path)

write_file(install_dir / 'coucou', (repo / 'windows/target/release/coucou').read_bytes(), 0o755)
write_file(launcher, ('#!/bin/sh\nexec ' + shlex.quote(str(install_dir / 'coucou')) + ' "$@"\n').encode(), 0o755)
write_file(icon, (repo / 'windows/src-tauri/icons/128x128.png').read_bytes())
# Desktop entries unescape string values before parsing Exec quoting.
def desktop_exec_path(path):
    text = str(path)
    if any(ord(char) < 32 or ord(char) == 127 or char == '=' for char in text):
        raise ValueError('This executable path cannot be represented in a desktop entry.')
    quoted = text.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
    return quoted.replace('\\', '\\\\')

exec_path = desktop_exec_path(launcher)
entry = ('[Desktop Entry]\nType=Application\nName=Coucou\n'
         'Comment=Mochi companion for local Codex sessions\n'
         f'Exec="{exec_path}"\nIcon=coucou\nTerminal=false\n'
         'Categories=Development;\nStartupNotify=false\n').encode()
write_file(desktop, entry)
if sys.argv[2] == '1':
    write_file(config / 'autostart/Coucou.desktop', entry)
    prefs = config / 'coucou/settings.json'
    # Preserve existing preferences; malformed settings stop the install here.
    values = json.loads(prefs.read_text()) if prefs.exists() else {
        'soundEnabled': True, 'soundVolume': 0.12, 'autoCloseInterval': 15,
        'absenceInterval': 180, 'activeIntegrations': [], 'screen': 'primary',
        'hooksInstalled': False, 'model': '',
    }
    values['autostart'] = True
    write_file(prefs, (json.dumps(values, indent=2) + '\n').encode(), 0o600)
print(f'Installed: {launcher}')
print(f'Applications menu: {desktop}')
if sys.argv[2] == '1':
    print(f'Autostart: {config / "autostart/Coucou.desktop"}')
print('Start with: coucou')
PY
if command -v update-desktop-database >/dev/null; then
  update-desktop-database "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
fi
