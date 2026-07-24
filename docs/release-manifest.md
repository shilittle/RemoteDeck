# RemoteDeck v1.0.0 release candidate artifacts

Generated on Windows x64 with Electron 43.2.0 and electron-builder 26.15.3. Files under `apps/desktop/release` are generated and intentionally not committed.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `RemoteDeck-1.0.0-win-x64-portable.exe` | 112,851,963 | `63d28c345f28791734dc7f1df68855346a6397354db22d897d8a5578fd9db5d7` |
| `RemoteDeck-1.0.0-win-x64-setup.exe` | 113,065,259 | `5cda4aff7ab47522d81bbef9e9fe05b36e84708a1270b8456250ebc7af6209ec` |
| `RemoteDeck-1.0.0-win-x64-setup.exe.blockmap` | 119,297 | `1e95d6262a6e10810836712b17dd8f0e900c049cf121999681c7f8ad56a49948` |

Local release evidence:

- `pnpm verify`: 53 unit tests and 4 integration tests passed, plus lint, TypeScript and production build.
- `pnpm test:e2e`: 2 Electron/Playwright tests passed.
- `pnpm dist:win`: NSIS and portable x64 targets passed.
- `m9-packaged-smoke.ps1`: portable window and isolated userData passed.
- `m9-clean-install.ps1`: clean temporary install, window launch and uninstall passed.
- `electron-fuses read`: runtime escape fuses disabled; cookie encryption/ASAR integrity enabled; file-protocol extra privileges disabled.
- `pnpm audit --audit-level high`: no known vulnerabilities.
