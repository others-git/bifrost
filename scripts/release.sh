#!/usr/bin/env bash
#
# release.sh — one-shot release: bump version, commit, gate, tag, push.
#
# Usage:  scripts/release.sh <version> <summary…>
#   e.g.  scripts/release.sh 0.19.0 "LLM voice fallback + kiosk logout hidden"
#   env:  SKIP_GATE=1  — skip the local test/lint gate (CI still runs on the tag)
#
# Order is deliberate so you can keep working while the gate runs:
#   1. Bump the canonical version in lockstep with the tag (the CLAUDE.md
#      invariant — a tag must never outrun the in-file version):
#        • Cargo.toml (+ Cargo.lock)  → Bifrost  (package `version`)
#        • app/build.gradle.kts       → kiosk    (versionName; versionCode +1)
#   2. COMMIT immediately — a fast checkpoint that frees the working tree, so the
#      gate (below) runs against a fixed snapshot while you edit on.
#   3. Run the test/lint gate (NOT a release build — that's GitHub Actions' job
#      on the pushed tag): Bifrost → fmt · clippy · test; kiosk → unit tests.
#   4. Only if the gate passes: annotated-tag `v<version>` and push branch + tag.
#
# If the gate fails the commit stays LOCAL (un-tagged, un-pushed). Fix, then
# `git commit --amend` and re-run — a clean tree already on the release commit is
# detected and the script resumes straight at the gate.
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
SUBJECT="Release $VERSION — $SUMMARY"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1 && die "tag $TAG already exists"

# ── detect repo ──────────────────────────────────────────────────────────────
if [ -f Cargo.toml ] && grep -q '^\[package\]' Cargo.toml; then
    KIND="Bifrost"
elif [ -f app/build.gradle.kts ]; then
    KIND="kiosk"
else
    KIND="unknown"
fi

# ── 1+2. bump & commit — unless we're resuming an already-committed release ──
if [ "$(git log -1 --pretty=%s)" = "$SUBJECT" ] && git diff --quiet && git diff --cached --quiet; then
    echo "release: resuming — $SUBJECT already committed; re-running gate then tag/push."
else
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

    git add -A
    git diff --cached --quiet && die "nothing to commit (already at $VERSION with no other changes?)"
    git commit -q -m "$SUBJECT" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
    echo "release: committed '$SUBJECT' (local) — working tree is free; running gate before tag/push…"
fi

# ── 3. test/lint gate (the authoritative build is GitHub Actions on the tag) ──
gate_fail() {
    echo "release: gate failed: $*" >&2
    echo "release: '$SUBJECT' is committed LOCALLY but was NOT tagged or pushed." >&2
    echo "release: fix it, then 'git commit --amend' and re-run (it resumes at the gate)," >&2
    echo "release: or 'git reset --soft HEAD~1' to unwind the release commit." >&2
    exit 1
}
if [ "${SKIP_GATE:-0}" = 1 ]; then
    echo "release: ⚠ test/lint gate skipped (SKIP_GATE=1) — relying on CI"
else
    case "$KIND" in
    Bifrost)
        echo "release: gate — cargo fmt · clippy · test…"
        cargo fmt --check                          || gate_fail "cargo fmt --check (run 'cargo fmt')"
        cargo clippy --all-targets -- -D warnings  || gate_fail "cargo clippy (-D warnings)"
        cargo test                                 || gate_fail "cargo test"
        ;;
    kiosk)
        echo "release: gate — ./gradlew testDebugUnitTest…"
        ./gradlew --no-build-cache testDebugUnitTest || gate_fail "gradle unit tests"
        ;;
    *)
        echo "release: no known gate for this repo — skipping" >&2
        ;;
    esac
    echo "release: gate passed ✓"
fi

# ── 4. tag · push ────────────────────────────────────────────────────────────
git tag -a "$TAG" -m "$SUBJECT"
git push -q origin "$BRANCH"
git push -q origin "$TAG"

echo "✅ $KIND $TAG — gate passed, committed, tagged, and pushed to origin/$BRANCH"
