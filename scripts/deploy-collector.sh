#!/usr/bin/env bash
# Maintainer tooling: builds the collector image on the server from the
# committed source.
#
#   scripts/deploy-collector.sh
#
# It sends schema/ and collector/ at HEAD (git archive) over ssh to
# `docker build` on PATHFINDER_DEPLOY_HOST (from the environment or
# scripts/release.env) and tags the image pathfinder-collector:<short sha>
# and :current. Restart the service that runs :current afterwards.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$root/scripts/release.env" ]] && source "$root/scripts/release.env"
host="${PATHFINDER_DEPLOY_HOST:-}"
[[ -n "$host" ]] || { echo "error: set PATHFINDER_DEPLOY_HOST (an ssh host with docker)" >&2; exit 1; }
if [[ -n "$(git -C "$root" status --porcelain -- schema collector)" ]]; then
    echo "error: schema/ or collector/ has uncommitted changes" >&2
    exit 1
fi
sha="$(git -C "$root" rev-parse --short HEAD)"
git -C "$root" archive --format=tar HEAD schema collector \
    | ssh "$host" "sudo docker build --build-arg COMMIT=$sha -f collector/Dockerfile -t pathfinder-collector:$sha -t pathfinder-collector:current -"
echo "built pathfinder-collector:$sha (tagged current) on $host"
