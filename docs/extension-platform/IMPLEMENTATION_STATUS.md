# Implementation status — Aurora Extension Platform v1

Current state: **IN PROGRESS — generic `.ext` package format, versioned API/SDK, import-free Wasmi sandbox, persistent user manager and viewer integration are being built. No production plugin is included. Renderer regression tests are passing after correcting a test fixture that assumed eye-to-water gap alone determined per-pixel depth.**

| Stage | State | Evidence | Remaining |
|---|---|---|---|
| 0 Reconnaissance / ADR | Partial | `ADR-001.md`; official instructions and existing ZIP validator reviewed; Wasmi 2.0.2 selected for interpreter-only, no-WASI runtime | Verify cross-platform targets, CVE process and dependency footprint |
| 1 Package / API | Implemented locally | Versioned API/SDK, bounded `.ext` ZIP/manifest validation, deterministic pack/validate CLI and adversarial tests | Security review, trust/authenticity model and cross-platform CI |
| 2 Runtime / lifecycle | Implemented locally | Import-free Wasmi execution with fuel/memory limits; explicit grants/revocation, persistent install/update/enable/disable/uninstall, worker, quarantine and safe mode | Review isolation assumptions, lifecycle coverage and failure recovery |
| 3 Viewer integration / UI | Implemented, targeted event tests pass | Preferences manager, package review, permission approvals, update flow, safe mode, native panels, accepted nearby-chat event adapter and host-rendered actions; the event-boundary tests pass | Verify the complete viewer workflow and inspect privacy/interaction behavior |
| 4 Hardening / cross-platform / human tests | In progress | Renderer, runtime, i18n and viewer event tests pass; latest viewer compile passes | Prescribed full checks, cross-platform CI evidence and human validation |
| 5 Upstream contribution | Not started | No issue or PR has been published | Owner approval, human validation, issue/board tracking and PR |

## Baseline and branch

- Base: official `Aurora-Viewer/Aurora-Viewer` `main`, commit `8a9d00ecd60e220d5e12a0bda99551d93f58c09a`.
- Local branch: `feat/extension-platform-v1` in an isolated worktree under `F:/extaurorawork/aurora-pr-worktree`.
- The fork's former `feature/tasia-extension-framework` is 536 commits behind current official main and differs by 1,566 files; it was not used as a base.
- No extension-related open issue was found by the initial search. No issue or PR has been published.

## Verification log

- `cargo test -p aurora-extension-runtime --locked`: 24 passed, 0 failed. This includes package bounds, generic API execution, consent/revocation, in-memory and persistent update reapproval, quarantine, and persistent restart/uninstall tests.
- `cargo test -p aurora-i18n --locked`: 26 passed, 0 failed across the library and catalog tests, including the new English/French/Japanese extension permission explanations.
- `cargo test -p aurora-viewer --locked extension_chat_tests -- --nocapture`: 3 passed, 0 failed. The tests cover accepted-other-resident input sanitization, event/annotation binding, and clearing stale chat targets.
- `cargo check -p aurora-viewer --locked`: passed after the latest update, host-action, and localized permission UI changes.
- The first viewer test run reported 9,068 passed, 26 failed, 22 ignored. Two failures came from this extension page: the theme guard rejected direct egui widgets and the icon test rejected the missing `puzzle-filled` asset. The page now uses shared Aurora widgets and the existing embedded `package` icon; both targeted tests pass individually. The remaining 24 failures are not yet classified.
- The final serial `scripts/check.ps1` run passed the PowerShell parser, agent hook, formatting, and workspace Clippy. The viewer test binary reported 9,073 passed, 21 failed, 22 ignored in 514.90 s; the script still exits 1 at the test step. The borrowed-row, Preferences-search, Confirmations, theme-guard, and icon tests pass individually after the extension page's Show-mode guard and shared-widget fixes.
- Offline viewer smoke test (2026-11-21): launched `AURORA_DEMO=1 AURORA_DEMO_OPTIONS=1 cargo run -p aurora-viewer -- --title 'Test extension-platform-v1 [AUTOMATION - DO NOT TOUCH]'`. The live window reached Preferences › Extensions; capture is `.scratch/extension-platform-smoke.png`. The viewer remains in demo mode; no grid was contacted.
- Viewer package review: the native file picker opened, accepted the generated `.ext`, and displayed a review for its ID and two requested permissions; capture is `.scratch/extension-package-review.png`. I left the package uninstalled and ungranted, so in-viewer install/enable/panel/notification behavior remains unverified.
- Generic demo artifact: `crates/aurora-extension-sdk/examples/demo_extension.rs` and its manifest request only `ui.panel.register` and `ui.notification.show`. Wasm32 build and `cargo clippy --workspace --all-targets --locked -- -D warnings` succeeded; CLI `pack` and `validate` succeeded; CLI content hash (manifest + module) is `3b6150ea180592ed6eece3f014cbd2c636d2d961bdaf1acad56897744829e1c2`; the delivered `.ext` file SHA-256 is `399f183c8e2ec45e82bfe41bed6c7993dcc19af6a6e0dd475591b25645688775`. A temporary harness loaded the actual `.ext` through `ExtensionManager` and Wasmi: install+enable succeeded, initialization registered the panel, its click action returned a notification, and safe mode blocked subsequent dispatch. Deliverable package: `F:/extaurorawork/extension-platform-demo/aurora-sandbox-demo.ext`.
- Clean-base spot checks on `8a9d00e…` confirm the two `ui::land` failures (53 passed, 2 failed) and representative `appearance::gallery`, `build::compact`, and blacklist-label failures reproduce on official base. The branch-only borrowed-row regression passed on base and now passes on this branch after guarding independent extension controls to Show mode. The other 21-run failures are in build UI, appearance, blacklist, locale/date formatting, radar, place details, scripts, sound explorer, groups, and RLVA privacy; they were not all compared individually against base. Windows culture is `pl-PL`.
- `cargo test -p aurora-render --locked`: 585 passed, 0 failed, 36 ignored. The water-depth test compares opaque-depth results against GPU-measured per-pixel depth from its clear-depth reference pass; its 4 m fixture covers the tested rays without the precision error from the former 200 m triangles. No production renderer or shader code changed.
- The renderer test and shader were unchanged from the worktree base before the test fix. `git log -S` traces the failing test to upstream commit `a9d0c5ce` (`feat(render): add experimental water effects and swimming AO controls`, PR #628).
- Earlier incremental viewer builds caught a missing summary accessor and `&str`/`String` mismatches in status messages; those were fixed. The latest viewer check passes. Cross-platform CI and human validation remain outstanding.
- Disk and launch side effects (2026-11-21): F:233 GB free (88% used), C:22 GB free (96%). The viewer log says the prior cache was removed when switching to the demo cache path, and it automatically fetched/installed the 240 MB media-plugins pack on C:. No grid credentials/login, paid service, or restart; avoid further large C: downloads.

## Acceptance gaps

The local framework now has a versioned API/SDK and package CLI; import-free Wasmi execution; explicit grants/revocation; persistent install/update/enable/disable/uninstall; trap/output quarantine; safe mode; a Preferences manager; worker-thread dispatch; sanitized nearby-chat input; validated notifications, annotations and host-rendered panels. Still outstanding: a publisher authenticity/trust model, full interactive validation of lifecycle/panel/notification workflows, final prescribed checks, cross-platform CI, and human validation. The WASM identity module remains a test fixture only; no production plugin is included. Do not publish a PR until the remaining checks and owner-controlled review are complete.
