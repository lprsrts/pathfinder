#!/usr/bin/env bash
# Maintainer tooling: builds, signs, notarises and publishes a release. You
# don't need it to build Pathfinder (`cargo build --release` in app/).
#
#   scripts/release.sh [notes-file]
#
# The notes default to this version's section of CHANGELOG.md. Settings come
# from the environment or scripts/release.env (see release.env.example):
#   PATHFINDER_SIGN_IDENTITY   codesigning identity; default: the one
#                              "Developer ID Application" identity in the keychain
#   PATHFINDER_NOTARY_PROFILE  notarytool keychain profile (required unless
#                              PATHFINDER_NOTARIZE=0, a dry run)
#   PATHFINDER_PUBLISH_TO      rsync target for the public folder (unset: build only)
#   PATHFINDER_COLLECTOR_URL   the live collector, checked before publishing
#   PATHFINDER_BUNDLE_ID       code-signing identifier (default com.lprsrts.pathfinder)
#
# The manifest is signed with the release key in the login keychain
# (`collector keygen` made it; `collector sign` uses it).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$root/scripts/release.env" ]] && source "$root/scripts/release.env"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/app/Cargo.toml" | head -1)"
dist="$root/dist"
out="$dist/public/releases"
notes_file="${1:-}"
profile="${PATHFINDER_NOTARY_PROFILE:-}"
notarize="${PATHFINDER_NOTARIZE:-1}"
publish_to="${PATHFINDER_PUBLISH_TO:-}"
collector_url="${PATHFINDER_COLLECTOR_URL:-}"
bundle_id="${PATHFINDER_BUNDLE_ID:-com.lprsrts.pathfinder}"
commit="$(git -C "$root" rev-parse HEAD)"

die() { echo "error: $*" >&2; exit 1; }

identity="${PATHFINDER_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
    identities="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p')"
    if [[ "$(wc -l <<<"$identities")" -ne 1 || -z "$identities" ]]; then
        die "expected exactly one Developer ID Application identity; set PATHFINDER_SIGN_IDENTITY"
    fi
    identity="$identities"
fi
echo "Pathfinder $version ($commit), signing as: $identity"

[[ -z "$(git -C "$root" status --porcelain -- app schema collector CHANGELOG.md)" ]] \
    || die "app/, schema/, collector/ or CHANGELOG.md has uncommitted changes; a release is built from a commit"

# Release notes: the file given, or this version's CHANGELOG section.
if [[ -n "$notes_file" ]]; then
    notes="$(cat "$notes_file")"
else
    notes="$(awk -v v="$version" '
        $0 ~ "^## " v "( |$)" { on = 1; next }
        on && /^## / { exit }
        on { print }
    ' "$root/CHANGELOG.md" | sed -e '/./,$!d')"
fi
[[ -n "${notes//[[:space:]]/}" ]] || die "no release notes: add a \"## $version\" section to CHANGELOG.md"

if [[ "$notarize" == 1 ]]; then
    [[ -n "$profile" ]] || die "set PATHFINDER_NOTARY_PROFILE (or PATHFINDER_NOTARIZE=0 for a dry run)"
    xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1 \
        || die "no notarytool profile \"$profile\""
fi

# The app sends events for a notice version; the live collector must accept it.
if [[ -n "$publish_to" ]]; then
    [[ -n "$collector_url" ]] || die "set PATHFINDER_COLLECTOR_URL to check the live collector before publishing"
    app_notice="$(sed -n 's/^pub const NOTICE_VERSION: u32 = \([0-9]*\);/\1/p' "$root/schema/src/lib.rs")"
    live_notice="$(curl -fsS "$collector_url/v1/version" | python3 -c 'import json,sys; print(json.load(sys.stdin)["notice"])' 2>/dev/null || echo 0)"
    (( live_notice >= app_notice )) \
        || die "the live collector accepts notice $live_notice, this app needs $app_notice: deploy the collector first (scripts/deploy-collector.sh)"
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
    (cd "$root/app" && cargo build --release --locked --target "$target")
done
mkdir -p "$dist/pkg"
bin="$dist/pkg/pathfinder"
lipo -create -output "$bin" \
    "$root/app/target/aarch64-apple-darwin/release/pathfinder" \
    "$root/app/target/x86_64-apple-darwin/release/pathfinder"
codesign --force --options runtime --timestamp --identifier "$bundle_id" --sign "$identity" "$bin"
codesign --verify --strict --verbose=2 "$bin"

if [[ "$notarize" == 1 ]]; then
    # A bare Mach-O is notarised inside a zip; it can't be stapled, so
    # Gatekeeper looks the ticket up online the first time it runs.
    zip="$(mktemp -d)/pathfinder.zip"
    ditto -c -k "$bin" "$zip"
    result="$(xcrun notarytool submit "$zip" --keychain-profile "$profile" --wait --output-format plist)"
    id="$(plutil -extract id raw - <<<"$result")"
    xcrun notarytool log "$id" --keychain-profile "$profile" "$dist/notary.json" >/dev/null
    [[ "$(plutil -extract status raw - <<<"$result")" == Accepted ]] \
        || die "notarisation failed; see $dist/notary.json"
    echo "notarised"
else
    echo "notarisation skipped (PATHFINDER_NOTARIZE=0): a dry run, not for distribution"
fi

# The archive holds the program as `pathfinder`, and as `harness` (a hard
# link, stored once) for apps before 0.3, which update by that name.
mac_tar="$version/pathfinder-$version-universal-apple-darwin.tar.gz"
ln "$bin" "$dist/pkg/harness"
cp "$root/LICENSE" "$root/NOTICE" "$dist/pkg/"
tar -czf "$out/$mac_tar" -C "$dist/pkg" pathfinder harness LICENSE NOTICE

# The source of exactly this build (AGPL).
src_tar="$version/pathfinder-$version-source.tar.gz"
git -C "$root" archive --format=tar.gz --prefix="pathfinder-$version/" -o "$out/$src_tar" HEAD

# The manifest, signed.
RELEASE_NOTES="$notes" python3 - "$out" "$version" "$mac_tar" "$src_tar" "$commit" <<'PY'
import hashlib, json, os, sys
out, version, mac_tar, src_tar, commit = sys.argv[1:]
def entry(rel):
    data = open(os.path.join(out, rel), "rb").read()
    return {"path": rel, "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
manifest = {
    "version": version,
    "files": {"universal-apple-darwin": entry(mac_tar), "source": entry(src_tar)},
    "notes": os.environ["RELEASE_NOTES"].strip(),
    "commit": commit,
}
json.dump(manifest, open(os.path.join(out, "latest.json"), "w"), indent=2)
PY
(cd "$root/collector" && CARGO_TARGET_DIR="$root/app/target" cargo run -q --release -- sign "$out/latest.json")

# The public folder: the pages and installer next to the releases.
for f in index.html install.sh favicon.svg changelog.html; do
    [[ -f "$root/collector/public/$f" ]] && cp "$root/collector/public/$f" "$dist/public/"
done
cp -R "$root/collector/public/privacy" "$dist/public/"

# Tag the release so it can be matched to its source.
if git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
    [[ "$(git -C "$root" rev-parse "v$version^{commit}")" == "$commit" ]] \
        || die "tag v$version already points at another commit"
else
    git -C "$root" tag -a "v$version" -m "Pathfinder $version" "$commit"
    echo "tagged v$version (push it with: git push origin v$version)"
fi

echo "release ready in $dist/public"
if [[ -n "$publish_to" ]]; then
    rsync -av "$dist/public/" "$publish_to/"
    echo "published to $publish_to"
fi
