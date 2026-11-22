#!/usr/bin/env bash
# Builds shadNet without Docker into <prefix>/bin: the executable, its default .cfg files, the Qt
# libraries it links (lib/) and the Qt plugins it loads (plugins/sqldrivers, plugins/tls).
# Qt 6.8 comes from aqtinstall because Ubuntu 24.04 only packages Qt 6.4.
#
#   bash build-native.sh <prefix> [work-dir]
#
# Needs git, cmake, ninja, g++ and python3 with venv. On Ubuntu:
#   apt-get install -y git cmake ninja-build g++ python3-venv libgl1-mesa-dev
set -euo pipefail

PREFIX="$(realpath -m "${1:?usage: build-native.sh <prefix> [work-dir]}")"
WORK="$(realpath -m "${2:-$PREFIX/build}")"
SHADNET_COMMIT="${SHADNET_COMMIT:-b38cf175ed2215456972356196f46b82c171e987}"
QT_VERSION="${QT_VERSION:-6.8.3}"
case "$(uname -m)" in
    x86_64) QT_HOST=linux; QT_ARCH=linux_gcc_64; QT_DIR=gcc_64 ;;
    aarch64) QT_HOST=linux_arm64; QT_ARCH=linux_gcc_arm64; QT_DIR=gcc_arm64 ;;
    *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac

mkdir -p "$WORK"
QT="$WORK/qt/$QT_VERSION/$QT_DIR"
if [ ! -d "$QT/lib" ]; then
    python3 -m venv "$WORK/aqt"
    "$WORK/aqt/bin/pip" install --quiet aqtinstall
    "$WORK/aqt/bin/aqt" install-qt "$QT_HOST" desktop "$QT_VERSION" "$QT_ARCH" \
        -m qthttpserver qtwebsockets -O "$WORK/qt"
fi

SRC="$WORK/shadNet"
[ -d "$SRC/.git" ] || git clone https://github.com/shadps4-emu/shadNet.git "$SRC"
git -C "$SRC" fetch --quiet origin
git -C "$SRC" checkout --quiet "$SHADNET_COMMIT"
git -C "$SRC" submodule update --init --recursive --depth 1

cmake -S "$SRC" -B "$SRC/build" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH="$QT"
cmake --build "$SRC/build" --parallel "$(nproc)" --target shadnet
strip "$SRC/build/shadnet"

mkdir -p "$PREFIX/bin/lib" "$PREFIX/bin/plugins/sqldrivers" "$PREFIX/bin/plugins/tls" "$PREFIX/bin/defaults"
cp -f "$SRC/build/shadnet" "$PREFIX/bin/shadnet"
for f in worlds.cfg scoreboards.cfg clientversions.cfg; do
    cp -f "$SRC/$f" "$PREFIX/bin/defaults/$f"
done
cp -f "$QT/plugins/sqldrivers/libqsqlite.so" "$PREFIX/bin/plugins/sqldrivers/"
cp -f "$QT/plugins/tls/libqopensslbackend.so" "$PREFIX/bin/plugins/tls/"
# Every library the executable and the plugins resolve from the Qt folder (Qt itself and its ICU).
LD_LIBRARY_PATH="$QT/lib" ldd "$PREFIX/bin/shadnet" "$PREFIX"/bin/plugins/*/*.so \
    | awk -v qt="$QT/lib/" 'index($3, qt) == 1 { print $3 }' | sort -u \
    | while read -r lib; do cp -fL "$lib" "$PREFIX/bin/lib/"; done
echo "shadNet $SHADNET_COMMIT with Qt $QT_VERSION installed in $PREFIX/bin"
