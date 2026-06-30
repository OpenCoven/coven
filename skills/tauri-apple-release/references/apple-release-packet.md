# Apple release packet reference

## Tauri iOS expected artifacts

Typical IPA path after Tauri iOS build:

```text
src-tauri/gen/apple/build/arm64/<AppName>.ipa
```

## Provisioning profile inspection

Useful commands:

```bash
security cms -D -i /path/to/AppStore.mobileprovision > /tmp/profile.plist
/usr/libexec/PlistBuddy -c 'Print :Entitlements:application-identifier' /tmp/profile.plist
/usr/libexec/PlistBuddy -c 'Print :Entitlements:get-task-allow' /tmp/profile.plist
/usr/libexec/PlistBuddy -c 'Print :ProvisionedDevices' /tmp/profile.plist 2>/dev/null || echo 'no provisioned devices'
/usr/libexec/PlistBuddy -c 'Print :DeveloperCertificates:0' /tmp/profile.plist >/tmp/profile-cert.der
openssl x509 -inform DER -in /tmp/profile-cert.der -noout -subject -fingerprint -sha1
```

Expected App Store profile:

- `get-task-allow` is false
- no provisioned devices
- `application-identifier` is `<TEAM_ID>.<BundleID>`
- developer cert fingerprint matches the installed Apple Distribution identity used for export

## Installed signing identities

```bash
security find-identity -v -p codesigning
```

Look for `Apple Distribution: <Org>` for iOS/App Store export, and `Developer ID Application: <Org>` for macOS direct download.

## Upload examples

API key mode:

```bash
xcrun altool --upload-app \
  --type ios \
  --file path/to/App.ipa \
  --api-key "$APPLE_API_KEY_ID" \
  --api-issuer "$APPLE_API_ISSUER" \
  --output-format json
```

Apple ID fallback:

```bash
xcrun altool --upload-package path/to/App.ipa \
  -u "$APPLE_ID" \
  -p "$APPLE_APP_SPECIFIC_PASSWORD" \
  --provider-public-id "$APPLE_PROVIDER_PUBLIC_ID" \
  --output-format json
```

## Error crib sheet

- `90161 Invalid Provisioning Profile`: archive/export/upload is using development/ad-hoc/wrong-profile signing. Need App Store distribution provisioning profile.
- Profile has right Bundle ID but old cert fingerprint: regenerate profile selecting currently installed Apple Distribution cert, or import private key for the old cert.
- API key works only with `--api-key-subject user`: treat as individual-subject upload auth; do not assume it can provision/export with xcodebuild.
