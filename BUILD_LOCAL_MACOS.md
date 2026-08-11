# Local macOS builds (fork-local)

How to run this fork as a real, installed app with working Accessibility,
instead of via `bun run tauri dev`. All new files — nothing upstream is
touched, per [ADDONS.md](ADDONS.md).

```bash
scripts/macos-dev-cert.sh                     # once, interactive — see "Stable identity"
scripts/build-macos-local.sh                  # .app + DMG under src-tauri/target/release/bundle/
scripts/install-macos-local.sh --link-models  # install to /Applications, reset TCC, launch
```

Both scripts read `productName` and `identifier` from `src-tauri/tauri.conf.json`
rather than hardcoding them, so a rebrand does not silently install the wrong
bundle.

## Why `tauri dev` can't get Accessibility

macOS TCC records every privacy grant against a bundle identifier **plus** a
code-signing requirement. Three things break in dev:

1. `tauri dev` produces a bare Mach-O at `src-tauri/target/debug/<name>`, not an
   `.app` bundle.
2. If the identifier matches an installed release build, that release's TCC
   record demands its Developer ID. A locally signed build cannot satisfy that
   requirement, so toggling Accessibility in System Settings appears to do
   nothing. Only one signature can hold a given identifier's record. Renaming
   the identifier sidesteps this entirely — a distinct identifier gets its own
   record and the two installs coexist.
3. `tauri.conf.json` sets `signingIdentity: "-"` (ad-hoc). An ad-hoc requirement
   pins the binary's cdhash, which changes on every rebuild, so even a fresh
   grant is stale by the next build.

`install-macos-local.sh` handles (1) and (2): it installs a real bundle and runs
`tccutil reset` for Accessibility, Input Monitoring (`ListenEvent`, which `rdev`
needs for the global shortcut), and Microphone, so the new signature can claim
those records. It refuses to overwrite a bundle at the destination whose
identifier differs from this build's.

## Stable identity

`scripts/macos-dev-cert.sh` fixes (3) by creating a self-signed code-signing
certificate. Signed with a real certificate, the requirement becomes
`identifier "..." and certificate leaf = H"<hash>"` — stable across rebuilds, so
Accessibility is granted once and `install-macos-local.sh` then preserves it
instead of resetting it.

This is optional. Without it the app works, but every rebuild needs a re-grant.
The script imports a certificate into your login keychain and marks it trusted
for code signing (`security add-trusted-cert -r trustRoot -p codeSign`) — a
persistent change to your machine's trust store, which is why it is a separate,
explicitly run step and may raise an authentication dialog. Equivalent by hand:
Keychain Access → Certificate Assistant → Create a Certificate, self-signed
root, type Code Signing.

Once the certificate exists, `build-macos-local.sh` picks it up automatically.
If it is created *after* a build, `install-macos-local.sh` re-signs the `.app`
in place rather than forcing a rebuild, but the DMG stays ad-hoc until you
rebuild.

## Model cache

The app-support directory is derived from the bundle identifier, so rebranding
the identifier means a fresh directory and re-downloading every model.
`--link-models` symlinks the new `models/` at the `com.pais.handy` one. Note
that this shares the cache: deleting a model in one app deletes it for both.
Copy the directory instead if you want them independent.

## Why not plain `bun run tauri build`

- `createUpdaterArtifacts: true` makes the updater bundle target demand
  `TAURI_SIGNING_PRIVATE_KEY`, which only exists in release CI; bundling fails
  without it. The build script forces it off and requests only `app` and `dmg`.
- rustup exports `cargo` from `~/.zshrc`, so any non-interactive shell dies at
  `cargo metadata`. The build script adds `~/.cargo/bin` to `PATH` itself.
- `bundle_dmg.sh` drives Finder over AppleScript to lay out the DMG window,
  which needs Apple Events automation permission. Without it the step exits 64
  and fails the whole bundle. Tauri passes `--skip-jenkins` when `CI` is set, so
  the script sets `CI` when stdout is not a TTY; interactive runs still get the
  laid-out DMG. It also clears any leftover `rw.*.dmg` from a failed run, which
  would otherwise break the next attempt.

## Notes

- The build is signed but not notarized. Locally built files carry no quarantine
  attribute so Gatekeeper stays out of the way; the install script strips it
  anyway.
- Two installed builds that both bind the same global shortcut will both fire on
  it. Quit one.
- Frontend-only work (addon styling, overlay visuals) does not need any of this
  — `bun run tauri dev` is still the fast loop. This path matters when you need
  transcription to actually type into other apps.
