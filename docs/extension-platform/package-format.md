# `.ext` package format (v1 draft)

An `.ext` file is a ZIP archive, not a native library. It contains exactly one `manifest.json`, the `module.wasm` entry module, and zero or more non-executable files under `assets/`.

```text
ExtensionFixture.ext
├── manifest.json
├── module.wasm
└── assets/icon.png                 # optional
```

Example manifest:

```json
{
  "manifest_version": 1,
  "id": "dev.example.extension-fixture",
  "name": "Extension Fixture",
  "version": "0.1.0",
  "api": ">=1.0.0, <2.0.0",
  "entry": "module.wasm",
  "permissions": [],
  "network_domains": [],
  "publisher": "Example developer"
}
```

The current validator accepts only manifest version 1, semantic versions and Rust `semver::VersionReq` API ranges compatible with the viewer's supplied API version (for example `>=1.0.0, <2.0.0`). See `manifest.schema.json` for the structural schema. Unknown manifest fields and permissions fail closed. The initial permission allowlist is `chat.nearby.read`, `ui.chat.annotate`, `ui.panel.register` and `ui.notification.show`; package metadata with non-empty `network_domains` is rejected until the network broker and consent UX exist. The parser does not grant permissions.

Archive limits in this draft: 32 MiB compressed input, 64 MiB expanded payload, 32 MiB per file, 128 ZIP entries, eight path components and a maximum 100:1 expansion ratio. The bounded end-of-central-directory preflight rejects over-limit counts and Zip64/multi-disk archives before the ZIP library constructs its entry index. Absolute paths, traversal, non-ASCII paths, backslashes, control characters, Windows reserved names (including space-padded device-name stems), duplicate/case-colliding paths, file/directory conflicts, symlinks and special files are rejected. Entries are validated and read into bounded memory; this crate does not extract paths to disk.

The validator computes SHA-256 over all files sorted by their UTF-8 archive path. For each file it hashes the little-endian 64-bit path byte length, path bytes, little-endian 64-bit content length, then content bytes. This is an integrity identifier, not a signature or proof of publisher identity. A signature/trust design is not implemented.

The current implementation validates ZIP structure and the manifest, then compiles/instantiates `module.wasm` with Wasmi 2.0.2. Imports, WASI and start functions are disabled; package validation checks the v1 `memory`, `alloc` and `handle` export signatures without invoking guest events. Each `GuestModule::handle_event` call uses the versioned JSON protocol described in `abi-v1.md`, validates the response against its request, and uses a fresh fuel-limited store. Per-instance limits are 16 MiB linear memory, 1,024 table elements and a 1 MiB value stack, with strict module compilation limits. Guest state does not persist between calls yet; installation, grants, lifecycle management and viewer UI are still incomplete. Only a minimal generic test fixture belongs in this contribution; production plugins are separate projects.
