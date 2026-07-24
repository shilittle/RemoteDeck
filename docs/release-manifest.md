# RemoteDeck v1.0.0 release candidate artifacts

Generated on Windows x64 with Electron 43.2.0 and electron-builder 26.15.3. Files under `apps/desktop/release` are generated and intentionally not committed.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `RemoteDeck-1.0.0-win-x64-portable.exe` | 112,851,817 | `d9a080130039ad861a778d6beba5430f1ea0b1aa0fd52e89edc3d3ecc4a81375` |
| `RemoteDeck-1.0.0-win-x64-setup.exe` | 113,065,119 | `6a754228f4a47e611df394f6187dc82be0cc0df5e10d1f46aba81ec6810c40f9` |
| `RemoteDeck-1.0.0-win-x64-setup.exe.blockmap` | 119,272 | `1d402e71b95717138a289993cd80cbe33cf6002b836233be6455fcfdb38d06b9` |

Local release evidence:

- `pnpm verify`: 52 unit tests and 4 integration tests passed, plus lint, TypeScript and production build.
- `pnpm test:e2e`: 2 Electron/Playwright tests passed.
- `pnpm dist:win`: NSIS and portable x64 targets passed.
- `m9-packaged-smoke.ps1`: portable window and isolated userData passed.
- `m9-clean-install.ps1`: clean temporary install, window launch and uninstall passed.
- `electron-fuses read`: runtime escape fuses disabled; cookie encryption/ASAR integrity enabled; file-protocol extra privileges disabled.
- `pnpm audit --audit-level high`: no known vulnerabilities.
