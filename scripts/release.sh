#!/usr/bin/env bash
#
# release.sh — one-shot release: CI gate, bump version, commit, tag, push.
#
# Usage:  scripts/release.sh <version> <summary…>
#   e.g.  scripts/release.sh 0.19.0 "LLM voice fallback + kiosk logout hidden"
#   env:  SKIP_GATE=1  — emergency escape hatch, skips the CI gate
#
# Detects the repo from its version file and:
#   1. Runs the repo's CI gate FIRST (before any change), aborting if it fails:
#        • Bifrost → cargo fmt --check · clippy -D warnings · test
#                    (cargo clean -p first — /mnt/d mtime skew can otherwise
#                     reuse a stale binary and pass tests against old code)
#        • kiosk   → ./gradlew testDebugUnitTest
#   2. Bumps the canonical version in lockstep with the tag (the CLAUDE.md
#      invariant — a tag must never outrun the in-file version):
#        • Cargo.toml (+ Cargo.lock)  → Bifrost  (package `version`)
#        • app/build.gradle.kts       → kiosk    (versionName; versionCode +1)
#   3. Stages everything, commits `Release <v> — <summary>`, annotated tag
#      `v<version>`, and pushes the branch + the tag to origin.
#
# Aborts cleanly (before any push) on: bad version, detached HEAD, an existing
# tag, a failed gate, or a failed bump — so a half-done release never ships.
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }

VERSION="${1:-}"; shift || true
SUMMARY="$*"
[ -n "$VERSION" ] || die "usage: $(basename "$0") <version> <summary…>"
[ -n "$SUMMARY" ] || die "a release summary is required (it becomes the commit subject)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like X.Y.Z (got '$VERSION')"

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "not inside a git repository"
cd "$ROOT"
BRANCH=$(git symbolic-ref --quiet --short HEAD) || die "detached HEAD — checkout a branch first"
TAG="v$VERSION"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1 && die "tag $TAG already exists"

# ── detect repo ──────────────────────────────────────────────────────────────
if [ -f Cargo.toml ] && grep -q '^\[package\]' Cargo.toml; then
    KIND="Bifrost"
elif [ -f app/build.gradle.kts ]; then
    KIND="kiosk"
else
    KIND="unknown"
fi

# ── CI gate (before any change, so a failure leaves the tree clean) ──────────
if [ "${SKIP_GATE:-0}" = 1 ]; then
    echo "release: ⚠ CI gate skipped (SKIP_GATE=1)"
else
    case "$KIND" in
    Bifrost)
        echo "release: CI gate — cargo fmt · clippy · test…"
        # /mnt/d mtime skew can make cargo reuse a stale binary and pass tests
        # against old code; force a fresh compile of our own crate.
        cargo clean -p bifrost
        cargo fmt --check                          || die "gate failed: cargo fmt --check (run 'cargo fmt')"
        cargo clippy --all-targets -- -D warnings  || die "gate failed: cargo clippy (-D warnings)"
        cargo test                                 || die "gate failed: cargo test"
        echo "release: CI gate passed ✓"
        ;;
    kiosk)
        echo "release: CI gate — ./gradlew testDebugUnitTest…"
        ./gradlew --no-build-cache testDebugUnitTest || die "gate failed: gradle unit tests"
        echo "release: CI gate passed ✓"
        ;;
    *)
        echo "release: no known CI gate for this repo — skipping" >&2
        ;;
    esac
fi

# ── bump the canonical version file ──────────────────────────────────────────
case "$KIND" in
Bifrost)
    NAME=$(sed -n 's/^name = "\(.*\)"/\1/p' Cargo.toml | head -1)
    # First `version = "…"` is the [package] one (it precedes any deps).
    sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" Cargo.toml
    grep -q "^version = \"$VERSION\"$" Cargo.toml || die "failed to bump Cargo.toml"
    # Keep Cargo.lock's own package entry in step — offline & deterministic.
    if [ -f Cargo.lock ] && [ -n "$NAME" ]; then
        perl -0pi -e "s/(name = \"\Q$NAME\E\"\nversion = \")[^\"]*/\${1}$VERSION/" Cargo.lock
    fi
    ;;
kiosk)
    CODE=$(grep -oP 'versionCode\s*=\s*\K[0-9]+' app/build.gradle.kts | head -1)
    [ -n "$CODE" ] || die "could not read versionCode from app/build.gradle.kts"
    sed -i "s/versionCode = $CODE/versionCode = $((CODE + 1))/" app/build.gradle.kts
    sed -i "s/versionName = \"[^\"]*\"/versionName = \"$VERSION\"/" app/build.gradle.kts
    grep -q "versionName = \"$VERSION\"" app/build.gradle.kts || die "failed to bump versionName"
    echo "release: versionCode $CODE → $((CODE + 1)), versionName → $VERSION"
    ;;
*)
    echo "release: no known version file — committing changes as-is" >&2
    ;;
esac

# ── commit · tag · push ──────────────────────────────────────────────────────
git add -A
git diff --cached --quiet && die "nothing to commit (already at $VERSION with no other changes?)"

git commit -q -m "Release $VERSION — $SUMMARY" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
git tag -a "$TAG" -m "Release $VERSION — $SUMMARY"
git push -q origin "$BRANCH"
git push -q origin "$TAG"

echo "✅ $KIND $TAG — gate passed, committed, tagged, and pushed to origin/$BRANCH"
