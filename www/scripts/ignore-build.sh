#!/bin/sh
# Vercel's Ignored Build Step for the website, named by www/vercel.json: exit 0 skips the
# build, and any other exit builds it.
#
# The website is built from www/, the workspace packages it imports and the workspace install.
# A deployment builds when one of them changed since its branch's last successful deployment,
# or, on a branch's first deployment, when they differ from main's; and whenever that cannot
# be told.

website="www web/fx-web web/fx-react package.json bun.lock"

cd "$(git rev-parse --show-toplevel)" || exit 1

base=${VERCEL_GIT_PREVIOUS_SHA:-}
if [ -z "$base" ]; then
  [ "${VERCEL_ENV:-}" = production ] && exit 1
  base=$(git ls-remote origin refs/heads/main | cut -f1)
  [ -n "$base" ] || exit 1
fi
# Vercel clones ten commits deep. A commit outside the clone is fetched on its own: fetching a
# ref the clone holds with --depth would cut the clone's history short.
git cat-file -e "$base^{commit}" 2>/dev/null || git fetch --quiet --depth=1 origin "$base" || exit 1

# shellcheck disable=SC2086 # the paths split into separate arguments
if git diff --quiet "$base" HEAD -- $website; then
  echo "Skipping the build: nothing the website is built from changed since $base."
  exit 0
fi
echo "Building: what the website is built from changed since $base."
exit 1
