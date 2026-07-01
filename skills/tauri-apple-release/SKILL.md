---
name: tauri-apple-release
description: Standardize, debug, or execute Tauri iOS TestFlight and macOS Apple release pipelines, including App Store Connect upload auth, Apple Distribution signing, provisioning profiles, notarization, bundle/version metadata, and single-packet release credential asks.
---

# Tauri Apple Release

Use this skill for Tauri iOS/TestFlight or macOS Apple release work.

## Prime directive

Treat Apple release work as **one release packet**, not a sequence of drip asks.

A usable packet includes:

1. **Team/app metadata** — Bundle ID, Team ID, version, build number, App Store Connect app record.
2. **Upload auth** — App Store Connect API key _or_ Apple ID app-specific password + provider public ID.
3. **Distribution signing** — Apple Distribution cert/private key and App Store provisioning profile for iOS; Developer ID cert + notarization auth for macOS direct download.
4. **Artifact path** — `.ipa`, `.app`, `.dmg`, etc.

Never ask for only “the issuer ID” unless the packet is otherwise complete and that exact field failed.

## Safety rules

- Do not print or paste private key, `.p8`, `.p12`, app-specific password, provisioning profile contents, or real secret values.
- It is okay to discuss non-secret identifiers: Bundle ID, Team ID, certificate SHA-1 fingerprint, profile UUID/name, key ID, issuer ID/provider public ID.
- Prefer 1Password/GitHub Secrets/env vars for secrets.
- Do not upload to TestFlight/App Store Connect until export validation passes with an App Store distribution provisioning profile.

## Standard triage

Start by identifying which gate failed:

1. **Build correctness** — `pnpm build`, `tauri build`, `tauri ios build` fails before Apple export/upload.
2. **Apple trust/signing** — archive/export fails, invalid provisioning profile, cert mismatch, wrong Bundle ID/Team ID.
3. **Release transport** — upload auth, provider/issuer mismatch, TestFlight processing.

Use exact Apple error numbers when present, e.g. `90161 Invalid Provisioning Profile`.

## Single release packet ask

When blocked on credentials/signing, ask once:

```text
Please provide/store ONE complete Apple release packet for <app>. Do not paste private key, certificate, provisioning profile, or app-specific password contents in chat.

App metadata:
- Bundle ID: <exact bundle id>
- Team ID: <Apple Developer Team ID>
- Version/build: <short version>/<build number>
- App Store Connect app record exists: yes/no

Upload auth — choose A or B:
A. Team App Store Connect API key:
   - APPLE_API_KEY_ID
   - APPLE_API_ISSUER
   - APPLE_API_KEY_PATH or APPLE_API_KEY_P8 in secrets
B. Apple ID fallback:
   - APPLE_ID
   - APPLE_APP_SPECIFIC_PASSWORD
   - APPLE_PROVIDER_PUBLIC_ID

Distribution signing — choose A, B, or C:
A. Xcode automatic signing account added on build Mac
B. Manual signing assets:
   - Apple Distribution .p12 + password
   - App Store provisioning profile for exact Bundle ID
C. Team ASC API key that xcodebuild can use for provisioning
```

## iOS/TestFlight workflow

1. Confirm metadata:
   - `src-tauri/tauri.conf.json > identifier`
   - Tauri version/build overrides if present
   - App Store Connect app record matches the exact Bundle ID
2. Build/archive:
   - `pnpm tauri ios build --export-method app-store-connect`
3. Verify profile type:
   - `get-task-allow = false`
   - no `ProvisionedDevices`
   - profile App ID matches `TEAM_ID.BundleID`
   - profile includes the certificate installed on the build Mac
4. Upload only after export is valid.

### Cert/profile mismatch rule

If the provisioning profile is for the right Bundle ID but includes an old cert fingerprint, and the Mac has a different usable Apple Distribution identity:

- Prefer regenerating/downloading the App Store provisioning profile selecting the current usable cert.
- Avoid reviving old cert private keys unless regeneration is impossible.

Example language:

```text
This is a cert/profile mismatch, not a general TestFlight failure. Regenerate the App Store provisioning profile for <Bundle ID> selecting the currently installed Apple Distribution cert <fingerprint>, then rerun export/upload.
```

## macOS direct-download workflow

1. Build signed app/DMG with Developer ID Application identity.
2. Notarize using App Store Connect credentials.
3. Staple and validate:
   - `codesign --verify --deep --strict path/to/App.app`
   - `spctl --assess --type execute --verbose path/to/App.app`
   - `xcrun stapler validate path/to/App.app`
   - `xcrun stapler validate path/to/App.dmg`

## Common Apple auth nuance

A `.p8` key can work for notarization or upload with `--api-key-subject user` but still fail automatic provisioning/export if it is not a team App Store Connect API key. For automated iOS export, use one of:

- Xcode account automatic signing,
- manual distribution cert + App Store profile,
- a team API key that `xcodebuild` can use for provisioning.

## Repo doctor

If the repo has a release doctor, run it before asking the user:

```bash
pnpm run release:credentials:doctor
```

If missing, add one or apply the single packet ask above manually.

## References

For deeper implementation details, read `references/apple-release-packet.md`.
