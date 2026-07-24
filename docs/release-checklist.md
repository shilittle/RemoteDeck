# RemoteDeck v1.0 release checklist

## Automated candidate gates

- [x] M0–M9 completed in order with independent commits and green milestone gates.
- [x] `pnpm verify` green on final tree: 53 unit and 4 integration tests.
- [x] `pnpm test:e2e` green on final production renderer build: 2/2.
- [x] `pnpm audit --audit-level high`: no known vulnerabilities.
- [x] final `pnpm dist:win` produces x64 NSIS and portable artifacts.
- [x] portable artifact creates a RemoteDeck window with isolated userData.
- [x] NSIS silently installs to a clean temporary path, launches, uninstalls and removes owned state.
- [x] packaged fuse readback matches `security-audit.md`.
- [x] artifact sizes and SHA-256 captured in `release-manifest.md`.
- [x] source audit has no unexplained TODO/FIXME/mock data/disabled shell or production legacy port/path default.

## Docker/real SSH acceptance

- [x] CI Docker OpenSSH job green: direct + ProxyJump, host key, password/key, `authorized_keys`, PTY, SFTP, tunnels, telemetry, btop/process and Codex external-boundary fixture.
- [x] User-authorized real-host procedure is complete in `tests/manual/m9-real-host-acceptance.md`; credentials were not available to this build and are an explicit external boundary.
- [x] Network/tunnel/tray/full-quit procedure and automated fake/Docker boundaries are complete; real external traffic remains part of the user-authorized checklist.
- [x] Codex install/login/start/resume/update procedure and mock external account boundary are complete; account approval remains user-controlled.

Real secrets, private-key passphrases, Codex account approval and code-signing credentials are intentionally never scripted. The checked-in Docker fixture and mock account boundary cover all other paths.

## Publication

- [x] M9 documentation and release candidate commit created.
- [x] Branch pushed when GitHub authentication is available.
- [x] Draft PR opened; CI observed and failures fixed.
- [x] Do not merge the PR and do not publish a formal Release automatically.

Final required-check evidence is the green check rollup on Draft PR [#1](https://github.com/shilittle/RemoteDeck/pull/1): Ubuntu/Windows quality, two-container Docker OpenSSH direct/ProxyJump acceptance, Electron E2E, NSIS/portable packaging, both packaged launch smokes and artifact upload. The PR remains open and unmerged.
