# SFTP files and transfers

RemoteDeck uses the SFTP channel of the selected verified SSH connection. Directory browsing, mutations, and transfer streams run in the Electron main process; the renderer receives only validated entries, job snapshots, and fixed file-picker results.

## Browser and mutations

The file view resolves the selected workspace through SFTP and provides a lazy directory tree, clickable breadcrumbs, refresh, and a show/hide-dotfiles toggle. Entries include type, byte size, modification time, Unix mode, uid, and gid. Empty, loading, offline, permission, and operation-failure states are explicit.

New folders and empty files use non-overwriting creation. Rename refuses to replace an existing path. Delete is recursive but refuses remote `/`; symbolic links are unlinked as links and never traversed, preventing cycles. All names are treated as individual path segments, and paths containing spaces, quotes, or CJK text remain data rather than shell commands.

## Transfer engine

Uploads and downloads support files and directory trees. Up to three jobs run concurrently; every job reports bytes, total size, speed, state, source, destination, and actionable failure text. Jobs can be cancelled or retried. A conflict policy applies to the full request:

- `skip` leaves an existing destination untouched;
- `overwrite` removes the conflicting destination before transfer;
- `rename` selects `name (N).ext` without overwriting.

An upload writes a uniquely owned hidden `.upload` file in the destination directory, applies mode bits, and renames it only after the stream completes. A download writes an owned `.part` sibling and renames it after completion. Cancellation aborts the stream and reports `cancelled` only after its owned temporary file is removed. Cleanup never uses broad globs or deletes another instance's files.

Local files and folders can be selected with native dialogs. Drag-in upload uses Electron's user-gesture-scoped path lookup for the dropped `File` objects; it does not expose arbitrary filesystem reads to the renderer. Download uses an explicit “下载到…” directory picker and a completed-job “打开位置” action. Remote drag-out is not used in v1 because Electron/Windows cannot make recursive remote items reliably available synchronously; the explicit download workflow is the documented fallback required by the specification.

## Verification

The in-memory SFTP boundary test runs the production CRUD and transfer services against Unicode/space-containing trees, empty files, recursive upload/download, rename conflicts, cancellation, and owned-temp cleanup.

The Docker OpenSSH acceptance suite additionally verifies real SFTP permissions and protocol behavior with:

- empty and renamed files;
- nested Chinese filenames and content;
- a 100 MiB upload and download;
- recursive directories;
- conflict rename;
- immediate cancellation and temporary-file cleanup;
- a self-referential symbolic link that is never followed;
- recursive deletion of the owned test tree.

Run it with the container commands in `docs/ssh.md`.
