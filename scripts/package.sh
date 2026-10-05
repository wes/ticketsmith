#!/bin/bash
# Builds a signed, notarized target/Ticketsmith-<version>.dmg, ready to download.
#
#   scripts/package.sh                 sign, package, notarize and staple
#   scripts/package.sh --no-notarize   sign and package, skip Apple
#
# scripts/release.sh runs this; on its own it is the way to check a change to
# the bundle still signs, without cutting a release.
#
#   TICKETSMITH_IDENTITY  the Developer ID, e.g.
#                         "Developer ID Application: Wes Edling (288BJX6YHP)"
#   NOTARY_PROFILE        the name given to `xcrun notarytool store-credentials`
#                         (default: ticketsmith)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
notarize=1
[ "${1:-}" = "--no-notarize" ] && notarize=0
profile=${NOTARY_PROFILE:-ticketsmith}

if [ -z "${TICKETSMITH_IDENTITY:-}" ]; then
	echo "package.sh: TICKETSMITH_IDENTITY is not set." >&2
	echo "Pick a \"Developer ID Application\" from: security find-identity -v -p codesigning" >&2
	exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -1)
app="$root/target/Ticketsmith.app"
dmg="$root/target/Ticketsmith-$version.dmg"

TICKETSMITH_UNIVERSAL=1 "$root/scripts/bundle.sh"

codesign --verify --deep --strict --verbose=2 "$app"

# The app beside a link to /Applications, so the window that opens is a
# drag-to-install.
staging=$(mktemp -d)
ditto "$app" "$staging/Ticketsmith.app"
ln -s /Applications "$staging/Applications"
rm -f "$dmg"
hdiutil create -volname "Ticketsmith $version" -srcfolder "$staging" \
	-ov -format ULFO "$dmg" >/dev/null
rm -rf "$staging"

# The image is signed too: the notarization ticket is stapled to it, and an
# unsigned container cannot carry one.
codesign --force --timestamp --sign "$TICKETSMITH_IDENTITY" "$dmg"

if [ "$notarize" = 0 ]; then
	echo "packaged $dmg (not notarized)"
	exit 0
fi

echo "Notarizing (usually a few minutes)…"
log="$root/target/notarize.log"
# Judged by the status it prints rather than its exit code, so a rejection
# still gets as far as fetching Apple's reasons below.
xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait --timeout 30m | tee "$log" || true
if ! grep -q "status: Accepted" "$log"; then
	id=$(grep -m1 -o 'id: [0-9a-f-]*' "$log" | cut -d' ' -f2 || true)
	echo "package.sh: notarization was not accepted." >&2
	[ -z "$id" ] || xcrun notarytool log "$id" --keychain-profile "$profile" >&2
	exit 1
fi

# Stapled, so the app opens on a Mac that is offline: the ticket travels in
# the image instead of being fetched from Apple.
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"

echo "packaged $dmg"
