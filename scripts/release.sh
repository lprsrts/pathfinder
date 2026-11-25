#!/usr/bin/env bash
# Builds, signs, notarises and publishes a Pathfinder release.
#
#   scripts/release.sh [notes-file]
#
# Environment:
#   PATHFINDER_SIGN_IDENTITY   signing identity; default: the one Developer ID
#                              Application identity in the keychain
#   PATHFINDER_NOTARY_PROFILE  notarytool keychain profile (default fringe-notary)
#   PATHFINDER_NOTARIZE=0      skip notarisation (a dry run, not for friends)
#   PATHFINDER_PUBLISH_TO      rsync target for the public folder; default
#                              maxwell:/srv/state/pathfinder/public (empty: don't publish)
#
# The manifest is signed with the release key from the login keychain
# (`collector keygen` made it; `collector sign` uses it).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/harness/Cargo.toml" | head -1)"
dist="$root/dist"
out="$dist/public/releases"
notes_file="${1:-}"
profile="${PATHFINDER_NOTARY_PROFILE:-fringe-notary}"
notarize="${PATHFINDER_NOTARIZE:-1}"
publish_to="${PATHFINDER_PUBLISH_TO-maxwell:/srv/state/pathfinder/public}"
bundle_id="com.lprsrts.pathfinder"

identity="${PATHFINDER_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
    identities="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p')"
    if [[ "$(wc -l <<<"$identities")" -ne 1 || -z "$identities" ]]; then
        echo "error: expected exactly one Developer ID Application identity; set PATHFINDER_SIGN_IDENTITY" >&2
        exit 1
    fi
    identity="$identities"
fi
echo "Pathfinder $version, signing as: $identity"

if [[ -n "$(git -C "$root" status --porcelain -- harness schema)" ]]; then
    echo "error: harness/ or schema/ has uncommitted changes; a release is built from a commit" >&2
    exit 1
fi
if grep -q CONTACT_EMAIL "$root/collector/public/privacy/index.html"; then
    echo "error: the privacy notice still has no contact address" >&2
    exit 1
fi
if [[ "$notarize" == 1 ]] && ! xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
    echo "error: no notarytool profile \"$profile\" (or set PATHFINDER_NOTARIZE=0 for a dry run)" >&2
    exit 1
fi

rm -rf "$dist"
mkdir -p "$out/$version"

# One universal binary for Apple silicon and Intel. Use rustup's toolchain
# (a Homebrew Rust earlier on PATH has no cross targets).
if command -v rustup >/dev/null; then
    toolchain_bin="$(dirname "$(rustup which rustc)")"
    export PATH="$toolchain_bin:$PATH"
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
fi
for target in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd "$root/harness" && cargo build --release --locked --target "$target")
done
bin="$dist/harness"
lipo -create -output "$bin" \
    "$root/harness/target/aarch64-apple-darwin/release/harness" \
    "$root/harness/target/x86_64-apple-darwin/release/harness"
codesign --force --options runtime --timestamp --identifier "$bundle_id" --sign "$identity" "$bin"
codesign --verify --strict --verbose=2 "$bin"

if [[ "$notarize" == 1 ]]; then
    # A bare Mach-O is notarised inside a zip; it can't be stapled, so
    # Gatekeeper looks the ticket up online the first time it runs.
    zip="$(mktemp -d)/harness.zip"
    ditto -c -k "$bin" "$zip"
    result="$(xcrun notarytool submit "$zip" --keychain-profile "$profile" --wait --output-format plist)"
    id="$(plutil -extract id raw - <<<"$result")"
    xcrun notarytool log "$id" --keychain-profile "$profile" "$dist/notary.json" >/dev/null
    if [[ "$(plutil -extract status raw - <<<"$result")" != Accepted ]]; then
        echo "error: notarisation failed; see $dist/notary.json" >&2
        exit 1
    fi
    echo "notarised"
else
    echo "notarisation skipped (PATHFINDER_NOTARIZE=0): not for distribution"
fi

mac_tar="$version/harness-$version-universal-apple-darwin.tar.gz"
cp "$root/LICENSE" "$root/harness/NOTICE" "$dist/"
tar -czf "$out/$mac_tar" -C "$dist" harness LICENSE NOTICE

# The source of exactly this build (AGPL).
src_tar="$version/pathfinder-$version-source.tar.gz"
git -C "$root" archive --format=tar.gz --prefix="pathfinder-$version/" -o "$out/$src_tar" HEAD

# The manifest, signed.
python3 - "$out" "$version" "$mac_tar" "$src_tar" "$notes_file" <<'PY'
import hashlib, json, os, sys
out, version, mac_tar, src_tar, notes_file = sys.argv[1:]
def entry(rel):
    data = open(os.path.join(out, rel), "rb").read()
    return {"path": rel, "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
notes = open(notes_file).read().strip() if notes_file else ""
manifest = {
    "version": version,
    "files": {"universal-apple-darwin": entry(mac_tar), "source": entry(src_tar)},
    "notes": notes,
}
json.dump(manifest, open(os.path.join(out, "latest.json"), "w"), indent=2)
PY
(cd "$root/collector" && CARGO_TARGET_DIR="$root/harness/target" cargo run -q --release -- sign "$out/latest.json")

# The public folder: installer and notice next to the releases.
cp "$root/collector/public/install.sh" "$dist/public/"
cp "$root/collector/public/index.html" "$dist/public/"
cp -R "$root/collector/public/privacy" "$dist/public/"

echo "release ready in $dist/public"
if [[ -n "$publish_to" ]]; then
    rsync -av "$dist/public/" "$publish_to/"
    echo "published to $publish_to"
fi
