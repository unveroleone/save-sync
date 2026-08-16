#!/usr/bin/env bash
# Repacks a Tauri-built Linux AppImage without the bundled GTK/WebKitGTK
# graphics stack.
#
# Tauri's AppImage bundler (linuxdeploy + gtk plugin) copies the build
# machine's whole GTK3/WebKitGTK/GStreamer/wayland library set into the
# bundle. Those Ubuntu 22.04 libs conflict with newer host graphics stacks
# (e.g. Bazzite/Fedora's Mesa), which fails EGL display creation with
# EGL_BAD_PARAMETER and leaves a blank white window. Forcing the host system
# libraries fixes it, so this script removes the bundled libs entirely and
# lets the app load webkit2gtk-4.1 etc. from the host -- exactly what the
# .deb/.rpm packages already do.
#
# Usage: strip-appimage.sh <input.AppImage> <output.AppImage>

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <input.AppImage> <output.AppImage>" >&2
    exit 1
fi

APPIMAGE="$(readlink -f "$1")"
OUTPUT="$(readlink -f "$2")"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"

# --appimage-extract works without FUSE, which GitHub runners do not have.
"$APPIMAGE" --appimage-extract >/dev/null
cd squashfs-root

# Drop every bundled system library. The AppImage runtime still puts the
# (now missing) usr/lib dirs on LD_LIBRARY_PATH; ld.so skips nonexistent
# directories, so the host libraries are used.
rm -rf usr/lib usr/lib64

# Replace the linuxdeploy gtk hook: it exported paths into the deleted
# bundled lib dirs (GTK_IM_MODULE_FILE, GTK_PATH, GIO_EXTRA_MODULES, ...)
# and forced the bundled Adwaita theme over the user's theme. Keep
# GDK_BACKEND=x11 (tauri#8541) and the schemas/icons that still ship
# in usr/share.
cat > apprun-hooks/linuxdeploy-plugin-gtk.sh <<'EOF'
#!/usr/bin/env bash

# Workaround to run extracted AppImage
export APPDIR="${APPDIR:-"$(dirname "$(realpath "$0")")"}"
# Crash with Wayland backend on Wayland - https://github.com/tauri-apps/tauri/issues/8541
export GDK_BACKEND=x11
# g_get_system_data_dirs() from GLib
export XDG_DATA_DIRS="$APPDIR/usr/share:/usr/share:$XDG_DATA_DIRS"
export GSETTINGS_SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"
EOF

# Repack. appimagetool itself is an AppImage; APPIMAGE_EXTRACT_AND_RUN
# avoids the FUSE requirement on CI runners.
if ! command -v appimagetool >/dev/null 2>&1; then
    curl -fsSL -o "$WORK/appimagetool" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
    chmod +x "$WORK/appimagetool"
    APPIMAGE_TOOL=("$WORK/appimagetool")
else
    APPIMAGE_TOOL=(appimagetool)
fi

APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 \
    "${APPIMAGE_TOOL[@]}" "$WORK/squashfs-root" "$OUTPUT"
