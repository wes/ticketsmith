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
