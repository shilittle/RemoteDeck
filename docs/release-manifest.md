# Release manifest record

No release artifact is asserted by this source document. The final values must be generated from the exact Windows x64 NSIS candidate by `scripts/package-win.ps1` and verified by `scripts/verify-release.ps1`.

The generated `dist/release-manifest.json` records at least:

```json
{
  "version": "2.1.0",
  "platform": "windows-x64",
  "installer": "RemoteDeck-2.1.0-win-x64-setup.exe",
  "sha256": "<lowercase sha256>",
  "bytes": 0,
  "authenticodeStatus": "NotSigned",
  "unsigned": true,
  "binary": "RemoteDeck.exe",
  "webAssets": "embedded in RemoteDeck.exe",
  "installerScope": "currentUser",
  "appData": "%APPDATA%\\io.github.shilittle.remotedeck"
}
```

`dist/SHA256SUMS.txt` must contain the same digest and exact installer file name. The candidate must pass clean installation, runtime descriptor/API readiness, independent process liveness, silent uninstall, user-data retention and the 40 MiB ceiling before a maintainer considers publication. A file path, manifest or CI message without the corresponding binary is not evidence of a release.

The current release workflow uploads a reviewable candidate artifact and does not publish a GitHub Release. Publication, signing and post-upload round-trip verification require explicit authorization and must be recorded separately.
