#!/usr/bin/env bash

set -e

cumulus_repo=$(cd "$(dirname "$0")" && git rev-parse --show-toplevel)
axia_repo=$(dirname "$cumulus_repo")/axia
if [ ! -d "$axia_repo/.git" ]; then
    echo "please clone axia in parallel to this repo:"
    echo "  (cd .. && git clone git@github.com:axiatech/axia.git)"
    exit 1
fi

if [ -z "$BRANCH" ]; then
    BRANCH=cumulus-branch
fi

cd "$axia_repo"
git fetch
git checkout "$BRANCH"
time docker build \
    -f ./docker/Dockerfile \
    --build-arg PROFILE=release \
    -t axia:"$BRANCH" .
