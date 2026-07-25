# RemoteDeck v1.0.1 release artifacts

Generated on Windows x64 with Electron 43.2.0 and electron-builder 26.15.3. The artifacts are intentionally unsigned (`Get-AuthenticodeSignature` reports `NotSigned`). Files under `apps/desktop/release` are generated and intentionally not committed.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `RemoteDeck-1.0.1-win-x64-portable.exe` | 112,852,050 | `dabcf20a26b0e78ec43884f0aa20a1924099a00da8045815d0b36b09a8fa6694` |
| `RemoteDeck-1.0.1-win-x64-setup.exe` | 113,065,350 | `209cf03ab20da62bb6208e737acffc2bb3f8935a11db6842b8f615a38e3b342d` |
| `RemoteDeck-1.0.1-win-x64-setup.exe.blockmap` | 119,306 | `8a2a7f96f0dd841de219f90e76043d7de4f45f81947a99cd401bef3c83615ce0` |

Local release evidence:

- `pnpm verify`: 53 unit tests and 4 integration tests passed, plus lint, TypeScript and production build.
- `pnpm test:e2e`: 2 Electron/Playwright tests passed.
- `pnpm dist:win`: NSIS and portable x64 targets passed.
- `m9-packaged-smoke.ps1`: portable window and isolated userData passed.
- `m9-clean-install.ps1`: clean temporary install, window launch and uninstall passed.
- `electron-fuses read`: runtime escape fuses disabled; cookie encryption/ASAR integrity enabled; file-protocol extra privileges disabled.
- `pnpm audit --audit-level high`: no known vulnerabilities.

Published-release evidence:

- [`v1.0.1`](https://github.com/shilittle/RemoteDeck/releases/tag/v1.0.1) is the latest non-draft, non-prerelease Release and points to tested commit `8ab1e27589fc2d19f8909ebe4f1304c355d9e11b`.
- GitHub's post-upload asset API reports the exact byte count and `sha256:` digest above for all three files.
- An authenticated Asset API stream independently re-downloaded the complete blockmap (119,306 bytes) and reproduced `8a2a7f96f0dd841de219f90e76043d7de4f45f81947a99cd401bef3c83615ce0`.
