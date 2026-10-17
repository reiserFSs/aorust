#!/bin/sh
# Clone a warm fast-profile target into a private Cargo target directory.
# Keep the reference read-only: no builds may modify it during this clone.
# Refresh it separately from main with --profile fast when needed.
# The reference must come from this aomac workspace namespace (ao-* and aomac).

fail() {
	printf '%s\n' "$*" >&2
	exit 1
}

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
	printf 'usage: %s DESTINATION\n' "$0" >&2
	printf 'Reference: AOMAC_FAST_TARGET_CACHE (default: $HOME/aomac-cache/target-fast)\n' >&2
	exit 2
fi

reference=${AOMAC_FAST_TARGET_CACHE:-${HOME:?HOME must be set}/aomac-cache/target-fast}
case $reference in
	/*) ;;
	*) reference=$PWD/$reference ;;
esac
[ -d "$reference/fast/deps" ] || fail "invalid reference (expected fast/deps): $reference"
reference=$(cd -P "$reference" && pwd -P) || fail "cannot resolve reference: $reference"

case $1 in
	/*) destination=$1 ;;
	*) destination=$PWD/$1 ;;
esac
# Resolve the parent before reserving the destination, including symlink aliases.
parent=$(cd -P "$(dirname "$destination")" && pwd -P) || fail "destination parent must exist: $destination"
destination=$parent/$(basename "$destination")
case $destination in
	"$reference"|"$reference"/*) fail "destination must be outside the reference: $destination" ;;
esac

# mkdir reserves a new path atomically; never clean a path we did not create.
mkdir "$destination" || fail "cannot create destination (must not already exist): $destination"
trap 'rm -rf "$destination"' 0
trap 'exit 1' HUP INT TERM

# macOS clonefile copy only; preserve Cargo fingerprint timestamps.
if ! cp -c -p -R "$reference/." "$destination/"; then
	fail "APFS clone failed: $reference -> $destination"
fi
# Workspace binaries can embed the reference worktree's CARGO_MANIFEST_DIR.
# Rebuild them for this worktree while retaining registry dependency caches.
rm -rf "$destination"/fast/.fingerprint/ao-* \
	"$destination"/fast/.fingerprint/aomac-* \
	|| fail "cannot invalidate workspace fingerprints: $destination"
trap - 0 HUP INT TERM
