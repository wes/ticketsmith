# Releasing

Releases are cut on a Mac, by hand, with one script. There is no CI step.

```sh
scripts/release.sh
```

It asks whether this is a patch, minor or major release, shows the notes and
what it is about to do, and on a yes:

1. sets the version in `Cargo.toml` and `Cargo.lock`;
2. builds arm64 and x86_64, `lipo`s them into one binary, signs the app and
   the disk image with the Developer ID, sends the image to Apple for
   notarization and staples the ticket (`scripts/package.sh`);
3. commits `Release <version>`, tags `v<version>` and pushes both;
4. creates the GitHub release with the disk image attached.

The notes are the `## <version>` section of `CHANGELOG.md` if there is one,
otherwise the commit subjects since the last tag. `CHANGELOG.md` is the one file
allowed to be uncommitted when the script starts, so it can be written for the
release and committed with it.

The disk image is uploaded twice: as `Ticketsmith-<version>.dmg`, and as
`Ticketsmith.dmg`, a name that never changes, so this link is always the newest
build:

```
https://github.com/wes/ticketsmith/releases/latest/download/Ticketsmith.dmg
```

It builds before it commits, so a failed build or notarization puts the version
back and pushes nothing; fix it and run it again. It refuses to run off `main`,
out of step with `origin/main`, or with anything but the changelog uncommitted.
A version that was tagged but never published is offered again, as long as the
app has not changed since the tag.

## What the Mac needs, once

- **The Developer ID certificate**, "Developer ID Application: Wes Edling
  (288BJX6YHP)", in the login keychain. The team is pinned so the script never
  picks a different certificate.
- **Notarization credentials** in the keychain. This asks for an app-specific
  password from [account.apple.com](https://account.apple.com):

  ```sh
  xcrun notarytool store-credentials ticketsmith \
      --apple-id wes@joedesigns.com --team-id 288BJX6YHP
  ```

- **Xcode**, for `metal`, `codesign`, `notarytool`, `stapler` and `hdiutil`;
  **rustup** (Homebrew's Rust has only one architecture); and **gh**, logged in
  with write access to `wes/ticketsmith`.

`TICKETSMITH_IDENTITY` and `NOTARY_PROFILE` override the certificate and the
keychain profile.

## Checking the packaging without releasing

```sh
TICKETSMITH_IDENTITY="Developer ID Application: Wes Edling (288BJX6YHP)" \
    scripts/package.sh --no-notarize
```

builds the universal, signed `target/Ticketsmith-<version>.dmg` and stops before
Apple. `scripts/bundle.sh` on its own is the everyday build: this Mac's
architecture, ad-hoc signed.

## The icon

`bundle/icon-1024.png` is drawn by `scripts/make-icon.swift`. Edit the script
and run `swift scripts/make-icon.swift bundle/icon-1024.png` to change it.

## How an installed copy updates itself

The status bar shows the running version. At launch, and every six hours while
it stays open, the app asks GitHub for the latest release
(`api.github.com/repos/wes/ticketsmith/releases/latest`). When that is newer, a
link appears beside the version:

1. **Update to <version>** downloads the disk image and checks it, and nothing
   is replaced yet. The app then says the update is ready.
2. **Restart to update** swaps the new app in for the old one in a single step
   (`renamex_np` with `RENAME_SWAP`) and relaunches. It is a second click
   because restarting clears the ticket on screen.

An update is refused unless all three of these hold for the app in the disk
image:

- its signature is intact (`codesign --verify --deep --strict`),
- its leaf certificate is team `288BJX6YHP`, since a valid Developer ID only
  proves that *somebody* signed it,
- Apple has notarized it (`spctl --assess`).

So a release is not only a download; it is what every installed copy will
fetch and run. The version comes from the tag, compared with the one compiled in
from `Cargo.toml`, and the script keeps the two in step. Drafts and prereleases
are ignored.

A copy that cannot replace itself shows **Download <version>**, which opens the
release page instead. That is a `cargo run` build, an app run straight from the
disk image, or one in a folder this user cannot write to.

To try the whole flow against the real latest release, run an installed copy
as if it were older:

```sh
TICKETSMITH_PRETEND_VERSION=0.0.1 /Applications/Ticketsmith.app/Contents/MacOS/ticketsmith
```
