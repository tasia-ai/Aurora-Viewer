# Communication UI improvements

This note records the communication ordering and per-window opacity changes on
the unpublished `feat/group-chat-texture-sharing` branch.

## Conversation ordering

- Drag bottom-dock thumbnails horizontally to reorder private IMs, group chats,
  and conferences. Existing activation, context-menu, unread-badge, and
  notification behavior remains attached to each session.
- Drag rows vertically in the Conversations window to set its independent
  display order. Incoming messages do not move a manually ordered row; newly
  opened sessions append to the end. Conference ID reassignment retains the
  session's position.
- Dock and Conversations ordering are stored separately and persist in
  `ui_layout.ron`.

## Per-window opacity

Right-click a Floater title bar to open its opacity slider and Reset option.
The override belongs to that individual window and persists in `ui_layout.ron`.
It changes the window's surface colors while keeping text, icons, and images
solid. High contrast and Disable glass continue to force opaque surfaces.

The control uses the shared Floater implementation, so it applies to Aurora
and Legacy styles. The new label is translated in English, French, and Japanese;
other locale files fall back to the English catalog entry.

## Verification status

Regression tests were added for saved ordering, independent dock/list order,
incoming-message stability, conference ID reassignment, unread positioning,
drag targets, window isolation, opacity reset/clamping, and layout
serialization. They have not been run in this workspace because Cargo, Rust,
and rustfmt are unavailable. `scripts/check.ps1` could not run because
PowerShell is unavailable. `git diff --check` passed. Manual UI smoke testing is
pending with the user.
