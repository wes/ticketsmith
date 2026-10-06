#!/bin/bash
# Cuts a release from this Mac: the version, the notes, a signed and notarized
# disk image, the tag, and the GitHub release.
#
#   scripts/release.sh
#
# It asks which kind of release this is, shows what it is about to do, and
# does nothing outward until you say yes. It builds before it commits: a build
# or a notarization that fails leaves nothing pushed and the version where it
# was, so it can simply be run again.
#
# Needs, once per machine (see RELEASING.md):
#   - the Developer ID certificate in the keychain,
#   - notarization credentials saved as a keychain profile:
#       xcrun notarytool store-credentials ticketsmith \
#           --apple-id wes@joedesigns.com --team-id 288BJX6YHP
#   - gh, logged in as someone who can write to wes/ticketsmith.
#
# TICKETSMITH_IDENTITY and NOTARY_PROFILE override the identity and profile.

set -euo pipefail

cd "$(dirname "$0")/.."

BRANCH=main
REPO=wes/ticketsmith
TEAM=288BJX6YHP

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

# --- what this Mac needs ----------------------------------------------------

[ "$(uname)" = "Darwin" ] || fail "Releases are built on a Mac."

[ -x "$HOME/.cargo/bin/rustup" ] ||
    fail "rustup is not installed. Install it from https://rustup.rs — Homebrew's Rust cannot build both architectures."
export PATH="$HOME/.cargo/bin:$PATH"

# The team is pinned, not just "a Developer ID", so the release is never signed
# with the wrong one of several certificates.
IDENTITY=${TICKETSMITH_IDENTITY:-$(security find-identity -v -p codesigning \
    | grep -o "\"Developer ID Application: [^\"]*($TEAM)\"" | head -1 | tr -d '"' || true)}
[ -n "$IDENTITY" ] || fail "No \"Developer ID Application: … ($TEAM)\" certificate in the keychain."

PROFILE=${NOTARY_PROFILE:-ticketsmith}
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 ||
    fail "No working notarization profile called \"$PROFILE\". Save one with:
  xcrun notarytool store-credentials $PROFILE --apple-id wes@joedesigns.com --team-id $TEAM"

gh auth status >/dev/null 2>&1 || fail "gh is not logged in. Run: gh auth login"

# --- where things stand -----------------------------------------------------

[ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] ||
    fail "Releases are cut from $BRANCH; this is $(git rev-parse --abbrev-ref HEAD)."

# Only the changelog may have uncommitted changes: it is the one file a
# release is expected to be written alongside. Anything else would ship in the
# release without anyone having meant it to.
stray=$(git status --porcelain | grep -v ' CHANGELOG.md$' || true)
[ -z "$stray" ] || fail "Commit or stash these first — only CHANGELOG.md may be uncommitted:
$stray"

git fetch --quiet origin "$BRANCH" --tags
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$BRANCH")" ] ||
    fail "$BRANCH is not the same as origin/$BRANCH. Pull or push first."

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[[ "$current" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "Could not read the version from Cargo.toml."
IFS=. read -r major minor patch <<<"$current"

tagged() { git rev-parse -q --verify "refs/tags/v$1" >/dev/null; }
published() { gh release view "v$1" --repo "$REPO" >/dev/null 2>&1; }

# --- which version ----------------------------------------------------------

bold "Ticketsmith is at $current."
options=()
if tagged "$current" && ! published "$current"; then
    # Tagged, and the build or the publish after it never finished. Built
    # from here rather than from the tag, which is only right while the app
    # itself has not changed since — so that is checked.
    if git diff --quiet "v$current" HEAD -- src bundle Cargo.toml Cargo.lock; then
        options+=("$current   — tagged but never published: build and publish it")
    else
        bold "v$current is tagged but was never published, and the app has changed since."
        echo "  Release a new version instead; $current will simply be skipped."
    fi
elif ! tagged "$current"; then
    # Never tagged, so never shipped: it can go out as it is — which is how
    # the first release goes.
    options+=("$current   — release the current version, never tagged")
fi
options+=(
    "$major.$minor.$((patch + 1))   — patch: fixes"
    "$major.$((minor + 1)).0   — minor: new things"
    "$((major + 1)).0.0   — major: breaking changes"
)

PS3="Which release? "
select choice in "${options[@]}"; do
    [ -n "${choice:-}" ] && break
done
next=${choice%% *}

if [ "$next" != "$current" ] && tagged "$next"; then
    fail "v$next is already tagged."
fi

# --- the notes --------------------------------------------------------------

# A "## $next" section in CHANGELOG.md, if one was written, is the notes.
# Otherwise they are the commit subjects since the last tag — nothing to write
# first.
notes=""
[ -f CHANGELOG.md ] &&
    notes=$(awk -v v="## $next" '$0 == v { on = 1; next } on && /^## / { exit } on' CHANGELOG.md)
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
    last=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
    notes=$(git log --no-merges --format='- %s' ${last:+"$last..HEAD"} | grep -v '^- Release [0-9]' || true)
    [ -n "$notes" ] || notes="Ticketsmith $next."
fi

# --- what is about to happen ------------------------------------------------

echo
bold "Release notes for $next:"
printf '%s\n' "$notes" | sed 's/^/  /'
echo
bold "About to:"
[ "$next" = "$current" ] || echo "  • set the version to $next in Cargo.toml and Cargo.lock"
echo "  • build both architectures, sign as $IDENTITY, and notarize (about 5–15 minutes)"
tagged "$next" || echo "  • commit \"Release $next\" (if anything changed), tag v$next, push both"
echo "  • publish Ticketsmith-$next.dmg (and a copy as Ticketsmith.dmg) to $REPO — installed copies are offered it"
echo
read -r -p "Go ahead? [y/N] " answer
[[ "$answer" =~ ^[Yy]$ ]] || { echo "Nothing done."; exit 1; }

# --- the build --------------------------------------------------------------

# Until the commit, a failure puts the version back, so the tree is as it was
# and this can be run again. The notes stay: they were worth writing.
committed=0
restore() {
    if [ "$committed" = 0 ] && [ "$next" != "$current" ]; then
        git checkout -- Cargo.toml Cargo.lock 2>/dev/null || true
        echo "The version is back at $current; nothing was committed or pushed." >&2
    fi
}
trap restore EXIT

if [ "$next" != "$current" ]; then
    sed -i '' "1,/^version = /s/^version = \"$current\"$/version = \"$next\"/" Cargo.toml
    # Brings this crate's own entry in Cargo.lock up to the new version
    # without upgrading a single dependency.
    cargo metadata --format-version 1 >/dev/null
fi

bold "Building and notarizing Ticketsmith ${next}…"
TICKETSMITH_IDENTITY="$IDENTITY" NOTARY_PROFILE="$PROFILE" scripts/package.sh
dmg=target/Ticketsmith-$next.dmg
[ -f "$dmg" ] || fail "The build finished without $dmg."

# What a downloader's Mac will check, checked here first.
spctl --assess --type open --context context:primary-signature "$dmg" ||
    fail "Gatekeeper does not accept $dmg."

# --- the tag ----------------------------------------------------------------

if ! tagged "$next"; then
    git add Cargo.toml Cargo.lock
    [ -f CHANGELOG.md ] && git add CHANGELOG.md
    git diff --cached --quiet || git commit -q -m "Release $next"
    committed=1
    git tag -a "v$next" -m "Ticketsmith $next"
    git push -q origin "$BRANCH"
    git push -q origin "v$next"
    bold "Pushed v$next."
else
    committed=1
fi

# --- the release ------------------------------------------------------------

notes_file=$(mktemp)
printf '%s\n' "$notes" >"$notes_file"
# The same disk image a second time under a name that never changes, so
# https://github.com/$REPO/releases/latest/download/Ticketsmith.dmg is always
# the newest build.
latest=target/Ticketsmith.dmg
cp "$dmg" "$latest"
gh release create "v$next" "$dmg" "$latest" \
    --repo "$REPO" \
    --title "Ticketsmith $next" \
    --notes-file "$notes_file" \
    --verify-tag \
    --latest
rm -f "$notes_file"

bold "Ticketsmith $next is out: https://github.com/$REPO/releases/tag/v$next"
echo "Latest download: https://github.com/$REPO/releases/latest/download/Ticketsmith.dmg"
