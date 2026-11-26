#!/usr/bin/env bash
# Maintainer tooling: publishes a GitHub release for a version that's tagged
# and built:
#
#   scripts/github-release.sh <version> <folder with that version's files>
#
# The notes are the version's section of CHANGELOG.md. Every file in the
# folder is attached (the archives, and from 0.3.1 the signed manifest), and
# install.sh. A
# release that already exists gets any missing files added. Needs `gh`,
# signed in to an account that can write to the repository.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:?usage: github-release.sh <version> <folder>}"
dir="${2:?usage: github-release.sh <version> <folder>}"
tag="v$version"

git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null || { echo "error: no tag $tag" >&2; exit 1; }
git -C "$root" ls-remote --exit-code --tags origin "$tag" >/dev/null || git -C "$root" push -q origin "$tag"

section="$(awk -v v="$version" '
    $0 ~ "^## " v "( |$)" { on = 1; next }
    on && /^## / { exit }
    on { print }
' "$root/CHANGELOG.md" | sed -e '/./,$!d')"
[[ -n "${section//[[:space:]]/}" ]] || { echo "error: no \"## $version\" section in CHANGELOG.md" >&2; exit 1; }
notes="$section

Install or update on a Mac: \`curl -fsSL https://pathfinder.lprsrts.com/install.sh | sh\`, or \`pathfinder update\`. The archive is signed and notarised by Apple; the installer and \`pathfinder update\` check it against the release manifest, which is signed with Pathfinder's release key."

shopt -s nullglob
# The installer too, so `releases/latest/download/install.sh` works while
# Pathfinder's server is down.
files=("$dir"/* "$root/collector/public/install.sh")
if gh release view "$tag" >/dev/null 2>&1; then
    gh release upload "$tag" "${files[@]}" --clobber >/dev/null
    echo "updated the GitHub release $tag"
else
    gh release create "$tag" "${files[@]}" --title "Pathfinder $version" --notes "$notes" --verify-tag >/dev/null
    echo "created the GitHub release $tag"
fi
