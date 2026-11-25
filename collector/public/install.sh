#!/bin/sh
# Installs Pathfinder:
#   curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh
#
# It downloads the newest release for this computer, checks it against the
# release manifest (and, on a Mac, that Apple's notarised signature is
# Pathfinder's), and puts `harness` in a folder for your own programs:
# PATHFINDER_BIN_DIR if set, else XDG_BIN_HOME, else ~/.local/bin.
# Nothing else changes.
set -eu

BASE="${PATHFINDER_URL:-https://pathfinder.lprsrts.com}"
TEAM_ID="N75CG6NPP4"

case "${XDG_BIN_HOME:-}" in
    /*) xdg_bin="$XDG_BIN_HOME" ;;
    *) xdg_bin="" ;;
esac
if [ -n "${PATHFINDER_BIN_DIR:-}" ]; then
    DEST="$PATHFINDER_BIN_DIR"
elif [ -n "$xdg_bin" ]; then
    DEST="$xdg_bin"
elif [ -n "${HOME:-}" ]; then
    DEST="$HOME/.local/bin"
else
    echo "HOME isn't set; say where to install with PATHFINDER_BIN_DIR=/some/folder." >&2
    exit 1
fi

case "$(uname -s)/$(uname -m)" in
    Darwin/*) target=universal-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-gnu ;;
    *)
        echo "Sorry, there's no Pathfinder build for $(uname -s) $(uname -m) yet." >&2
        exit 1
        ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$BASE/releases/latest.json" -o "$tmp/latest.json"
# The manifest is indented JSON: the target's block holds path and sha256.
field() {
    awk -v t="\"$target\"" -v f="\"$1\"" '
        index($0, t) { inside = 1; next }
        inside && index($0, f) { sub(/.*: *"/, ""); sub(/".*/, ""); print; exit }
        inside && /}/ { exit }
    ' "$tmp/latest.json"
}
path="$(field path)"
sha="$(field sha256)"
version="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -1)"
if [ -z "$path" ] || [ -z "$sha" ]; then
    echo "Sorry, release $version has no build for this computer ($target)." >&2
    exit 1
fi

echo "Downloading Pathfinder $version…"
curl -fsSL "$BASE/releases/$path" -o "$tmp/release.tar.gz"
if command -v shasum >/dev/null 2>&1; then
    got="$(shasum -a 256 "$tmp/release.tar.gz" | cut -d' ' -f1)"
else
    got="$(sha256sum "$tmp/release.tar.gz" | cut -d' ' -f1)"
fi
if [ "$got" != "$sha" ]; then
    echo "The download doesn't match the release manifest; stopping." >&2
    exit 1
fi
tar -xzf "$tmp/release.tar.gz" -C "$tmp"

if [ "$target" = universal-apple-darwin ]; then
    if ! codesign --verify --strict "$tmp/harness" 2>/dev/null ||
        ! codesign -dv "$tmp/harness" 2>&1 | grep -q "TeamIdentifier=$TEAM_ID"; then
        echo "The binary isn't signed by Pathfinder's developer (team $TEAM_ID); stopping." >&2
        exit 1
    fi
fi

mkdir -p "$DEST"
chmod 755 "$tmp/harness"
mv "$tmp/harness" "$DEST/harness"
echo "Installed: $DEST/harness ($version)"

case ":$PATH:" in
    *":$DEST:"*)
        found="$(command -v harness 2>/dev/null || true)"
        if [ -n "$found" ] && [ "$found" != "$DEST/harness" ]; then
            echo
            echo "Note: another harness comes first on your PATH ($found)."
            echo "Remove it, or run Pathfinder as $DEST/harness."
        fi
        ;;
    *)
        echo
        echo "$DEST isn't on your PATH yet. To add it:"
        case "$(basename "${SHELL:-sh}")" in
            zsh) echo "  echo 'export PATH=\"$DEST:\$PATH\"' >> \"${ZDOTDIR:-${HOME:-\$HOME}}/.zshrc\" && exec zsh" ;;
            bash)
                if [ "$(uname -s)" = Darwin ]; then rc="${HOME:-\$HOME}/.bash_profile"; else rc="${HOME:-\$HOME}/.bashrc"; fi
                echo "  echo 'export PATH=\"$DEST:\$PATH\"' >> \"$rc\" && exec bash"
                ;;
            fish) echo "  fish_add_path \"$DEST\"" ;;
            *) echo "  echo 'export PATH=\"$DEST:\$PATH\"' >> \"${HOME:-\$HOME}/.profile\"  (then log in again)" ;;
        esac
        ;;
esac

cat <<'NEXT'

Next:
  1. Your model account: harness auth login chatgpt
  2. Start, in the folder you're working in:
                         cd ~/path/to/project && harness tui
     The first start asks whether to share usage data (it's optional),
     then you sign in with your email in the browser.
  3. Later updates:      harness update

Privacy notice: https://pathfinder.lprsrts.com/privacy
NEXT
