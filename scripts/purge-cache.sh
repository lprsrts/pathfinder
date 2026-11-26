#!/usr/bin/env bash
# Maintainer tooling: drops the edge's copies of the installer and the
# top-level release manifest, so the next request gets the new ones.
#
#   scripts/purge-cache.sh [path ...]     (default: the installer and manifest)
#
# Settings come from the environment or scripts/release.env:
#   PATHFINDER_COLLECTOR_URL     the public address, e.g. https://pathfinder.example.com
#   PATHFINDER_CF_ZONE           the Cloudflare zone id
#   PATHFINDER_CF_TOKEN_SERVICE  the macOS keychain item holding an API token
#                                with Cache Purge permission
# Without PATHFINDER_CF_ZONE it does nothing. The token reaches curl on
# stdin, never on a command line.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$root/scripts/release.env" ]] && source "$root/scripts/release.env"
zone="${PATHFINDER_CF_ZONE:-}"
base="${PATHFINDER_COLLECTOR_URL:-}"
service="${PATHFINDER_CF_TOKEN_SERVICE:-}"
if [[ -z "$zone" ]]; then
    echo "no PATHFINDER_CF_ZONE: not purging the edge cache"
    exit 0
fi
[[ -n "$base" && -n "$service" ]] || { echo "error: set PATHFINDER_COLLECTOR_URL and PATHFINDER_CF_TOKEN_SERVICE" >&2; exit 1; }
token="$(security find-generic-password -s "$service" -w)"

paths=("$@")
[[ ${#paths[@]} -gt 0 ]] || paths=(/install.sh /releases/latest.json /releases/latest.json.sig)
files="$(printf '%s\n' "${paths[@]}" | python3 -c 'import json,sys; base=sys.argv[1]; print(json.dumps({"files": [base + p.strip() for p in sys.stdin if p.strip()]}))' "$base")"

out="$(printf 'header = "Authorization: Bearer %s"\n' "$token" \
    | curl -sS --config - -X POST -H 'Content-Type: application/json' --data-binary "$files" \
        "https://api.cloudflare.com/client/v4/zones/$zone/purge_cache")"
if [[ "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("success"))')" != True ]]; then
    echo "error: the purge failed: $out" >&2
    exit 1
fi
echo "purged from the edge cache: ${paths[*]}"
