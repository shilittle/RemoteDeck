# RemoteDeck v1.0.1 release checklist

## Automated candidate gates

- [x] Clean frozen dependency installation succeeds.
- [x] `pnpm verify` is green on the final v1.0.1 tree: 53 unit and 4 integration tests.
- [x] `pnpm test:e2e` is green on the final production renderer build: 2/2.
- [x] `pnpm audit --audit-level high` reports no known vulnerabilities.
- [x] `pnpm dist:win` produces the unsigned x64 NSIS and portable artifacts.
- [x] The portable artifact creates a RemoteDeck window with isolated userData.
- [x] NSIS silently installs to a clean temporary path, launches, uninstalls, and removes owned state.
- [x] Packaged fuse readback matches `security-audit.md`.
- [x] Artifact sizes and SHA-256 are captured in `release-manifest.md`.
- [x] Source audit has no unexplained TODO/FIXME/mock data/disabled shell or production legacy port/path default.

## Docker/real SSH acceptance

- [x] CI Docker OpenSSH job green: direct + ProxyJump, host key, password/key, `authorized_keys`, PTY, SFTP, tunnels, telemetry, btop/process and Codex external-boundary fixture.
- [x] User-authorized real-host procedure is complete in `tests/manual/m9-real-host-acceptance.md`; credentials were not available to this build and are an explicit external boundary.
- [x] Network/tunnel/tray/full-quit procedure and automated fake/Docker boundaries are complete; real external traffic remains part of the user-authorized checklist.
- [x] Codex install/login/start/resume/update procedure and mock external account boundary are complete; account approval remains user-controlled.

Real secrets, private-key passphrases, and Codex account approval are intentionally never scripted. The checked-in Docker fixture and mock account boundary cover all other paths.

## Publication

- [x] Explicit user authorization received on 2026-07-25 to publish an unsigned v1.0.1 directly to public `main`.
- [x] Final v1.0.1 commit is pushed to `main` after all candidate checks pass.
- [x] All required GitHub Actions jobs are green on the exact release commit: run [`30159429692`](https://github.com/shilittle/RemoteDeck/actions/runs/30159429692).
- [x] The `v1.0.1` tag and bilingual Release point to the exact tested `main` commit `8ab1e27589fc2d19f8909ebe4f1304c355d9e11b`.
- [x] NSIS, portable, and blockmap assets are uploaded with exact SHA-256 values.
- [x] GitHub's post-upload size and SHA-256 digest match all three local assets; authenticated Asset API streaming independently revalidated the complete blockmap.

The v1.0.0 baseline evidence remains in Draft PR [#1](https://github.com/shilittle/RemoteDeck/pull/1). This checklist records the independently rerun v1.0.1 patch validation and the explicit decision to distribute unsigned Windows assets with published SHA-256 values.
