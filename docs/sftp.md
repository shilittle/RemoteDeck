# SFTP and transfers

Rust drives system `sftp.exe` in batch mode over the same strict app-owned host trust as every other connection. Batch commands use bounded quoting and reject controls/newlines. Listings preserve spaces and Unicode while limiting entry/output sizes.

The UI supports browsing, parent navigation, create, rename, recursive delete, native local file/folder selection, native Tauri drag-drop paths, recursive upload/download, and open-in-folder for completed local results. Background operations require key/agent authentication.

Transfers have bounded concurrency and explicit queued/running/cancelling/completed/failed/cancelled states. Conflict behavior is ask, overwrite, skip, or deterministic rename. Uploads and downloads first write an app-owned temporary path. An overwrite moves the existing destination to an app-owned backup, promotes the complete temporary result, then removes the backup; failed promotion attempts rollback, and a failed rollback preserves and reports the backup path instead of silently losing the original. Cancellation can clean only temporary paths bearing the current job's valid UUID ownership suffix. Destructive remote roots, traversal, relative ambiguity, basename escape, oversized trees, and symlink-recursion hazards are rejected.

Retry is not bound to the stale profile captured by the failed attempt. It requires the selected host to own the job, resolves the current saved host profile, revalidates the operation and limits, replaces the captured route, and only then queues a new attempt.

Every top-level SFTP CRUD operation and every multi-step transfer transaction holds a per-host operation barrier. Deleting a host first retires new SFTP work, cancels transfers, waits for both job controls and direct SFTP operations to become idle, and purges retained jobs so they cannot be retried after deletion. Connection-critical edits to either the target profile or its direct ProxyJump route are rejected while queued, running, or cancelling transfers still use that route; harmless presentation-only edits remain possible.
