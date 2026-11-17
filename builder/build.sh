#!/bin/bash
# Clones a repository at a branch or commit and builds it into a local image.
# A Dockerfile wins; otherwise Railpack detects the language and framework.
#
# Inputs (env): JD_REPO_URL JD_IMAGE [JD_BRANCH] [JD_COMMIT] [JD_ROOT_DIR]
#   [JD_BUILD_TYPE auto|dockerfile|railpack] [JD_DOCKERFILE] [JD_GIT_TOKEN]
#   [JD_BUILD_ENV_KEYS "A B C" naming service variables also present in the env]
set -euo pipefail

: "${JD_REPO_URL:?}" "${JD_IMAGE:?}"
branch="${JD_BRANCH:-main}"
root="${JD_ROOT_DIR:-}"
build_type="${JD_BUILD_TYPE:-auto}"
dockerfile="${JD_DOCKERFILE:-Dockerfile}"
read -r -a keys <<<"${JD_BUILD_ENV_KEYS:-}"

mkdir -p /work && cd /work
git init -q
if [ -n "${JD_GIT_TOKEN:-}" ]; then
  # A header rather than a URL credential, so the token never appears in output.
  auth=$(printf 'x-access-token:%s' "$JD_GIT_TOKEN" | base64 | tr -d '\n')
  git config http.extraHeader "Authorization: Basic $auth"
fi
git remote add origin "$JD_REPO_URL"
if [ -n "${JD_COMMIT:-}" ]; then
  echo "==> Fetching $JD_REPO_URL at ${JD_COMMIT:0:12}"
  git fetch -q --depth 1 origin "$JD_COMMIT"
else
  echo "==> Fetching $JD_REPO_URL ($branch)"
  git fetch -q --depth 1 origin "refs/heads/$branch"
fi
git checkout -q FETCH_HEAD
echo "::jd-commit:: $(git rev-parse HEAD) $(git log -1 --format=%s)"

if [ -n "$root" ]; then
  if [ ! -d "$root" ]; then
    echo "Root directory '$root' does not exist in the repository"
    exit 1
  fi
  cd "$root"
fi

if [ "$build_type" = "dockerfile" ] || { [ "$build_type" = "auto" ] && [ -f "$dockerfile" ]; }; then
  echo "==> Building with $dockerfile"
  args=()
  for key in "${keys[@]}"; do args+=(--build-arg "$key"); done
  docker build --progress=plain -f "$dockerfile" "${args[@]}" -t "$JD_IMAGE" .
else
  echo "==> No Dockerfile; building with Railpack $RAILPACK_VERSION"
  prepare=()
  secrets=()
  for key in "${keys[@]}"; do
    prepare+=(--env "$key=${!key}")
    secrets+=(--secret "id=$key,env=$key")
  done
  railpack prepare . --plan-out /tmp/railpack-plan.json --info-out /tmp/railpack-info.json "${prepare[@]}"
  # Rebuild layers that read a variable when its value changes.
  hash=$(for key in "${keys[@]}"; do printf '%s=%s\n' "$key" "${!key}"; done | sha256sum | cut -c1-16)
  docker build --progress=plain \
    --build-arg BUILDKIT_SYNTAX="ghcr.io/railwayapp/railpack-frontend:v$RAILPACK_VERSION" \
    --build-arg secrets-hash="$hash" \
    -f /tmp/railpack-plan.json "${secrets[@]}" -t "$JD_IMAGE" .
fi
echo "==> Built $JD_IMAGE"
