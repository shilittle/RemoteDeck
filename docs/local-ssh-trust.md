# Reusing existing local SSH trust

RemoteDeck previously imported host profiles while leaving its dedicated
`known_hosts` empty. This made a host already trusted by local OpenSSH fail with
`No ... host key is known` until the user repeated fingerprint confirmation.

Startup, host save and config import now copy matching existing records from the
current user's standard `.ssh/known_hosts` into previously unpinned app endpoints.
Existing profiles are included, and a trust file created after service startup is
discovered when a host is saved or imported. Save/import are tracked operations:
HTTP returns an operation ID immediately and repeated request IDs do not duplicate
the work. Global SSH files remain unchanged; existing app pins take precedence.

Exact addresses, non-default ports and hashed records are supported. Revoked or
unsupported marked trust is never converted into ordinary keys. This is a snapshot
of established trust; custom `UserKnownHostsFile`, `HostKeyAlias`, certificates,
wildcard config and `Include` support are not added by this change.

Release v2.1.1 validation on Windows, 2026-09-09, Rust 1.98.0:

- Frontend lint, typecheck and 49 unit tests passed; production build passed.
- Rust workspace: 177 core tests, 34 server tests and 2 Windows lifecycle tests
  passed. Strict Clippy and formatting passed.
- All 5 explicit OpenSSH integration tests passed against the disposable Docker
  fixture, including Windows system `ssh -G` validation.
- Exact v2.1.1 browser rerun: **4 passed (27.1s)**. The new real OpenSSH flow creates
  an isolated local hashed trust file after service startup, imports a config,
  tests the connection and uses/reopens a ConPTY terminal without another scan or
  acceptance request. It verifies that the source trust file remains unchanged.
- Existing first-use refusal/acceptance, command execution, real file transfer,
  local tunnel and terminal replay/exclusive-control browser flows also passed.

Logs are local ignored artifacts in `.cache/release-2.1.1-*`; earlier development
reruns are in `.cache/local-ssh-trust-*`. The release manifest and annotated tag
identify the exact published source and installer. External Agent/monitoring
conditions and injected transfer rollback coverage remain subject to the existing
release checklist.
