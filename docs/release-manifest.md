# RemoteDeck 2.0.0 release artifacts

No RemoteDeck 2 artifact is asserted by this document yet. Final values will be recorded only from the exact Windows x64 NSIS release candidate after clean install, launch, uninstall, Authenticode-status inspection, and independent checksum verification. Generated binaries remain outside Git; only after every release gate passes may the GitHub Release publish the installer together with `SHA256SUMS.txt` and `release-manifest.json`.

| Artifact | Bytes | SHA-256 | Signature |
| --- | ---: | --- | --- |
| `RemoteDeck-2.0.0-win-x64-setup.exe` | Pending exact build | Pending exact build | Pending inspection |

Required evidence before publication:

- final source commit and `v2.0.0` tag;
- complete frontend and Rust gate output/counts, including explicit skips and their external prerequisites;
- NSIS build path and size below 40 MiB;
- unpackaged and clean-installed window smoke results;
- silent uninstall result;
- Authenticode status;
- local SHA-256, uploaded byte count/digest, and independent post-upload download hash;
- GitHub Actions URLs for the exact release commit/tag.

The release configuration accepts no portable or blockmap artifact: RemoteDeck 2 deliberately targets one lightweight NSIS installer using the system WebView2 and Windows OpenSSH. This target description does not fill the pending evidence above.
