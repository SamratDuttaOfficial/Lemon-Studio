#!/usr/bin/env bash
# Copyright (C) 2026 Samrat Dutta
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Builds Lemon Studio and writes a Windows installer to dist/ at the repository root. Run it from Git Bash:
#
#   tools/installer/build.sh [--no-plugins]
#
#   1. Installs any missing prerequisites with winget (rustup, LLVM, VS Build Tools + C++ workload, Inno Setup).
#   2. Builds lemonstudio.exe and Lemon Studio's VST3/CLAP plugins with the MSVC Rust toolchain.
#   3. Compiles tools/installer/lemonstudio.iss into dist/LemonStudio-Setup-<version>-<commit>.exe (older installers
#      are removed).
#
#   --no-plugins  leave the VST3/CLAP plugins out of the installer (Lemon Studio has them built in either way)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
STAGE="$REPO/target/installer/stage"
DIST="$REPO/dist"
ISS="$HERE/lemonstudio.iss"
TOOLCHAIN=stable-x86_64-pc-windows-msvc

PLUGINS=1
for arg in "$@"; do
    case "$arg" in
        --no-plugins) PLUGINS=0 ;;
        -h|--help) sed -n '6,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

step() { printf '\n\033[36m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

PF="$(cygpath -u "$PROGRAMFILES")"
PF86="$(cygpath -u "$(printenv 'ProgramFiles(x86)')")"
LAD="$(cygpath -u "$LOCALAPPDATA")"
export PATH="$HOME/.cargo/bin:$PATH"

find_vctools() {
    local vswhere="$PF86/Microsoft Visual Studio/Installer/vswhere.exe"
    [ -x "$vswhere" ] || return 1
    [ -n "$("$vswhere" -products '*' -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)" ]
}

find_libclang() {
    local dir
    for dir in "${LIBCLANG_PATH:+$(cygpath -u "$LIBCLANG_PATH")}" "$PF/LLVM/bin" "$PF86/LLVM/bin"; do
        [ -n "$dir" ] && [ -f "$dir/libclang.dll" ] && { echo "$dir"; return 0; }
    done
    return 1
}

find_iscc() {
    local f
    for f in "$LAD"/Programs/Inno\ Setup*/ISCC.exe "$PF86"/Inno\ Setup*/ISCC.exe "$PF"/Inno\ Setup*/ISCC.exe; do
        [ -f "$f" ] && { echo "$f"; return 0; }
    done
    return 1
}

# ensure <name> <winget id> <test command> [winget args...]
# winget exits non-zero when the package is already installed, so success is judged by the test afterwards.
ensure() {
    local name="$1" id="$2" test="$3"; shift 3
    if $test >/dev/null 2>&1; then echo "  $name found"; return; fi
    echo "  Installing $name ($id)..."
    winget install --id "$id" -e --accept-source-agreements --accept-package-agreements "$@" || true
    $test >/dev/null 2>&1 || die "$name could not be installed. Install it manually and run this script again."
}

# ----------------------------------------------------------------------------------------------------------------------
step 'Checking prerequisites'
ensure rustup                 Rustlang.Rustup                        'command -v rustup'
ensure LLVM                   LLVM.LLVM                              find_libclang
ensure 'VS Build Tools (C++)' Microsoft.VisualStudio.2022.BuildTools find_vctools \
    --override '--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
ensure 'Inno Setup'           JRSoftware.InnoSetup                   find_iscc
LIBCLANG_PATH="$(cygpath -w "$(find_libclang)")"
export LIBCLANG_PATH
ISCC="$(find_iscc)"

step "Selecting the $TOOLCHAIN toolchain"
rustup toolchain install "$TOOLCHAIN" --profile minimal

VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$REPO/Cargo.toml" | head -n1)"
COMMIT="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo local)"

cd "$REPO"
step "Building Lemon Studio $VERSION ($COMMIT)"
cargo "+$TOOLCHAIN" build --release -p os-app
if [ "$PLUGINS" = 1 ]; then
    step 'Building the VST3 and CLAP plugins'
    cargo "+$TOOLCHAIN" run --release -p bundle-plugins
fi

# ----------------------------------------------------------------------------------------------------------------------
step 'Staging files'
rm -rf "$STAGE"
mkdir -p "$STAGE/app" "$STAGE/VST3" "$STAGE/CLAP"
cp "$REPO/target/release/lemonstudio.exe" "$STAGE/app/"
cp "$REPO/LICENSE" "$STAGE/app/LICENSE.txt"
cp "$REPO/THIRD-PARTY-NOTICES.txt" "$STAGE/app/"
if [ "$PLUGINS" = 1 ]; then
    shopt -s nullglob
    for p in "$REPO"/target/bundled/*.vst3; do cp -r "$p" "$STAGE/VST3/"; done
    for p in "$REPO"/target/bundled/*.clap; do cp -r "$p" "$STAGE/CLAP/"; done
    shopt -u nullglob
fi

step 'Compiling the installer'
mkdir -p "$DIST"
rm -f "$DIST"/LemonStudio-Setup-*.exe
# MSYS would rewrite the /D... switches as paths.
MSYS_NO_PATHCONV=1 "$ISCC" /Q \
    "/DAppVersion=$VERSION" \
    "/DCommit=$COMMIT" \
    "/DStage=$(cygpath -w "$STAGE")" \
    "/DRepo=$(cygpath -w "$REPO")" \
    "/DOutDir=$(cygpath -w "$DIST")" \
    "/DWithPlugins=$PLUGINS" \
    "$(cygpath -w "$ISS")"

printf '\n\033[32mDone: %s\033[0m\n' "$(cygpath -w "$(ls "$DIST"/LemonStudio-Setup-*.exe)")"
