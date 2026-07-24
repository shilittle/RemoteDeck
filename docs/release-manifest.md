# RemoteDeck v1.0.0 release candidate artifacts

Generated on Windows x64 with Electron 43.2.0 and electron-builder 26.15.3. Files under `apps/desktop/release` are generated and intentionally not committed.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `RemoteDeck-1.0.0-win-x64-portable.exe` | 112,851,712 | `68a6702c6666448c5dd9a16797fbf5296a07e3f8893eeb1c78063a54e2fe89e4` |
| `RemoteDeck-1.0.0-win-x64-setup.exe` | 113,065,119 | `32c1e026f8f44fe6abac7d709861dab8f42515af1f3138ac80c70eb89f30efc3` |
| `RemoteDeck-1.0.0-win-x64-setup.exe.blockmap` | 119,305 | `695d66f6cccf9c0cc1fa2104884d6cb4c8e855f43a99a742b36b85eec9e52578` |

Local release evidence:

- `pnpm verify`: 52 unit tests and 4 integration tests passed, plus lint, TypeScript and production build.
- `pnpm test:e2e`: 2 Electron/Playwright tests passed.
- `pnpm dist:win`: NSIS and portable x64 targets passed.
- `m9-packaged-smoke.ps1`: portable window and isolated userData passed.
- `m9-clean-install.ps1`: clean temporary install, window launch and uninstall passed.
- `electron-fuses read`: runtime escape fuses disabled; cookie encryption/ASAR integrity enabled; file-protocol extra privileges disabled.
- `pnpm audit --audit-level high`: no known vulnerabilities.
