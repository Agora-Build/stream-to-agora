#!/usr/bin/env bash
#
# scripts/release.sh — prepare a release locally.
#
# USAGE
#   ./scripts/release.sh              # patch-bump (0.1.0 → 0.1.1)
#   ./scripts/release.sh 0.2.0        # explicit version
#
# WHAT IT DOES
#   1. Resolves target version (auto patch-bump if no argument)
#   2. Checks: tag doesn't exist, working tree is clean (except Cargo.toml/lock)
#   3. Updates Cargo.toml
#   4. Runs `cargo build` to refresh Cargo.lock (rolls back Cargo.toml on failure)
#   5. Creates commit "chore(release): vX.Y.Z"
#   6. Creates tag vX.Y.Z locally
#
# WHAT IT DOES NOT DO
#   - Does NOT push (run `git push && git push origin vX.Y.Z` yourself after review)
#   - Does NOT run tests (run `cargo test` first)
#
# AFTER RUNNING
#   git show HEAD                          # review the release commit
#   git push && git push origin vX.Y.Z     # publish (triggers GitHub Actions release)

set -euo pipefail

cd "$(dirname "$0")/.."

CARGO_TOML="Cargo.toml"

red()   { printf "\033[31m%s\033[0m" "$1"; }
green() { printf "\033[32m%s\033[0m" "$1"; }
dim()   { printf "\033[2m%s\033[0m" "$1"; }

die() {
    red "error: "; echo "$1"
    exit 1
}

CURRENT="$(grep -m1 '^version' "$CARGO_TOML" | sed -E 's/version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')"
[[ -n "$CURRENT" ]] || die "could not read current version from $CARGO_TOML"

if [[ "$#" -ge 1 ]]; then
    TARGET="$1"
else
    [[ "$CURRENT" =~ ^([0-9]+\.[0-9]+\.)([0-9]+)$ ]] \
        || die "current version '$CURRENT' is not X.Y.Z — pass an explicit version"
    TARGET="${BASH_REMATCH[1]}$((BASH_REMATCH[2] + 1))"
fi

[[ "$TARGET" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "target version '$TARGET' is not X.Y.Z"
[[ "$TARGET" == "$CURRENT" ]] && die "target version is the same as current ($CURRENT). Bump required."

TAG="v$TARGET"
if git rev-parse "$TAG" >/dev/null 2>&1; then
    die "tag $TAG already exists"
fi

echo "$(dim "Current:"  ) $CURRENT"
echo "$(dim "Target: " ) $(green "$TARGET")"
echo "$(dim "Tag:    " ) $TAG"
echo

DIRTY="$(git status --porcelain | grep -v -E '^\s*M\s+(Cargo\.toml|Cargo\.lock)$' || true)"
if [[ -n "$DIRTY" ]]; then
    red "error: "; echo "working tree has uncommitted changes (other than Cargo.toml/Cargo.lock):"
    echo "$DIRTY" | sed 's/^/  /'
    echo
    echo "Commit or stash them first."
    exit 1
fi

echo "Updating $CARGO_TOML..."
sed -i.bak -E "0,/^version[[:space:]]*=/{s/^(version[[:space:]]*=[[:space:]]*)\"[^\"]+\"/\1\"$TARGET\"/}" "$CARGO_TOML"
rm -f "$CARGO_TOML.bak"

NEW_VER="$(grep -m1 '^version' "$CARGO_TOML" | sed -E 's/version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')"
[[ "$NEW_VER" == "$TARGET" ]] || die "Cargo.toml update failed (still $NEW_VER)"

echo "Building to refresh Cargo.lock..."
if ! cargo build >/dev/null 2>&1; then
    red "error: "; echo "cargo build failed"
    git checkout -- "$CARGO_TOML"
    exit 1
fi

git add Cargo.toml Cargo.lock
git commit -m "chore(release): v$TARGET

🤖 Built with SMT <smt@agora.build>
"

git tag "$TAG"

echo
green "Done."; echo " Committed and tagged $TAG (not pushed)."
echo
echo "Next steps:"
echo "  $(dim "# review the commit")"
echo "  git show HEAD"
echo "  $(dim "# push both branch and tag when ready")"
echo "  git push && git push origin $TAG"
