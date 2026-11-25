#!/usr/bin/env bash
# Builds the collector image on Maxwell from the committed source:
#   scripts/deploy-collector.sh
# It sends schema/ and collector/ at HEAD (git archive) to `docker build` over
# ssh and tags the image pathfinder-collector:<short sha> and :current. The
# stack in maxwell-infra (stacks/pathfinder) runs :current; restart it with
#   ssh maxwell 'cd /srv/maxwell-infra/stacks/pathfinder && sudo docker compose up -d'
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -n "$(git -C "$root" status --porcelain -- schema collector)" ]]; then
    echo "error: schema/ or collector/ has uncommitted changes" >&2
    exit 1
fi
sha="$(git -C "$root" rev-parse --short HEAD)"
git -C "$root" archive --format=tar HEAD schema collector \
    | ssh maxwell "sudo docker build -f collector/Dockerfile -t pathfinder-collector:$sha -t pathfinder-collector:current -"
echo "built pathfinder-collector:$sha (tagged current)"
