#!/bin/bash
# Assembles target/Ticketsmith.app.
#
#   scripts/bundle.sh          builds release and assembles target/Ticketsmith.app
#   scripts/bundle.sh --open   and launches it
#
# Two environment variables change what comes out; scripts/package.sh sets
# both. Without them it is a local build: this Mac's architecture, ad-hoc signed.
#
#   TICKETSMITH_IDENTITY   the codesigning identity. Defaults to `-`, ad-hoc,
#                          which is enough to run it here; a download needs the
#                          Developer ID.
#   TICKETSMITH_UNIVERSAL  set to 1 to build arm64 and x86_64 and lipo them
#                          into one binary.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
app="$root/target/Ticketsmith.app"
contents="$app/Contents"
identity=${TICKETSMITH_IDENTITY:--}

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -1)
[ -n "$version" ] || { echo "bundle.sh: no version in Cargo.toml" >&2; exit 1; }

if [ "${TICKETSMITH_UNIVERSAL:-0}" = "1" ]; then
	# rustup's cargo, not Homebrew's: a Homebrew Rust earlier on PATH has only
	# this Mac's target, and the error for the missing one tells you to add the
	# target rustup already has.
	[ -x "$HOME/.cargo/bin/rustup" ] || {
		echo "bundle.sh: a universal build needs rustup (https://rustup.rs)." >&2
		exit 1
	}
	export PATH="$HOME/.cargo/bin:$PATH"
	rustup --quiet target add aarch64-apple-darwin x86_64-apple-darwin

	for target in aarch64-apple-darwin x86_64-apple-darwin; do
		cargo build --release --target "$target" --manifest-path "$root/Cargo.toml"
	done
	binary=$(mktemp -d)/ticketsmith
	lipo -create -output "$binary" \
		"$root/target/aarch64-apple-darwin/release/ticketsmith" \
		"$root/target/x86_64-apple-darwin/release/ticketsmith"
else
	cargo build --release --manifest-path "$root/Cargo.toml"
	binary="$root/target/release/ticketsmith"
fi

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$binary" "$contents/MacOS/ticketsmith"

cp "$root/bundle/Info.plist" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$contents/Info.plist"

iconset=$(mktemp -d)/Ticketsmith.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
	sips -z $size $size "$root/bundle/icon-1024.png" \
		--out "$iconset/icon_${size}x${size}.png" >/dev/null
	sips -z $((size * 2)) $((size * 2)) "$root/bundle/icon-1024.png" \
		--out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/Ticketsmith.icns"
rm -rf "$(dirname "$iconset")"

# The hardened runtime is on even for the ad-hoc signature: notarization
# requires it, and finding out what it forbids belongs on the desk, not the
# release. A secure timestamp only exists for a real identity.
timestamp=--timestamp
[ "$identity" != "-" ] || timestamp=--timestamp=none
codesign --force "$timestamp" --options runtime \
	--sign "$identity" --identifier com.joedesigns.ticketsmith "$app"

echo "built $app ($version)"

if [ "${1:-}" = "--open" ]; then
	open "$app"
fi
