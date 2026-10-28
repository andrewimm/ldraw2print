#!/bin/sh
# Usage: packaging/release-check.sh v0.1.0
#
# Release archives take their version from Cargo.toml (`ldraw2print --version`
# and the archive names), never from the tag they were built at, so the two
# can disagree silently: a v0.2.0 tag would happily produce 0.1.0 binaries.
# Run before building a release.
set -eu

tag=${1:?usage: release-check.sh <tag, such as v0.1.0>}
root=$(cd "$(dirname "$0")/.." && pwd)

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
if [ "$tag" != "v$version" ]; then
    echo "tag $tag would build version $version, which is released as v$version;" \
        "set one to match the other" >&2
    exit 1
fi

echo "$tag matches the package version"
