#!/usr/bin/env bash
# Build Telegram's official JSON client and install it for this user only.
set -euo pipefail
[[ "$(uname -s)" == Linux ]] || { echo 'This installer requires Linux.' >&2; exit 1; }
revision=42e6a5259551178d1dab54a22ad96d14bd906e20
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/coucou/tdlib/$revision"
jobs="${COUCOU_BUILD_JOBS:-4}"
[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || { echo 'COUCOU_BUILD_JOBS must be a positive integer.' >&2; exit 2; }
for tool in git cmake ninja clang clang++ gperf pkg-config python3; do
  command -v "$tool" >/dev/null || { echo "Missing build dependency: $tool (see docs/TELEGRAM.md)" >&2; exit 1; }
done
pkg-config --exists openssl zlib || { echo 'OpenSSL and zlib development files are required.' >&2; exit 1; }
mkdir -p "$cache_dir"
if [[ ! -d "$cache_dir/source/.git" ]]; then
  git init -q "$cache_dir/source"
  git -C "$cache_dir/source" remote add origin https://github.com/tdlib/td.git
fi
if ! git -C "$cache_dir/source" cat-file -e "$revision^{commit}" 2>/dev/null; then
  git -C "$cache_dir/source" fetch --depth 1 origin "$revision"
fi
git -C "$cache_dir/source" checkout --detach "$revision"
cmake -S "$cache_dir/source" -B "$cache_dir/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
  -DTD_ENABLE_LTO=OFF
cmake --build "$cache_dir/build" --target tdjson --parallel "$jobs"
python3 - "$cache_dir" "$revision" <<'PY'
import datetime
from pathlib import Path
import shutil
import sys

cache = Path(sys.argv[1])
destination = Path.home() / '.local/lib/coucou'
destination.mkdir(parents=True, exist_ok=True)
stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
for source, name in [(cache / 'build/libtdjson.so', 'libtdjson.so'),
                     (cache / 'source/LICENSE_1_0.txt', 'TDLib-LICENSE.txt')]:
    target = destination / name
    if target.exists():
        shutil.copy2(target, target.with_name(target.name + '.backup-' + stamp))
    temporary = target.with_name(target.name + '.tmp-' + stamp)
    shutil.copyfile(source, temporary)
    temporary.chmod(0o644)
    temporary.replace(target)
(destination / 'TDLib-revision.txt').write_text(sys.argv[2] + '\n')
print(f'Installed Telegram runtime: {destination / "libtdjson.so"}')
print('Open Coucou Settings → Telegram → Open Telegram to sign in.')
PY
