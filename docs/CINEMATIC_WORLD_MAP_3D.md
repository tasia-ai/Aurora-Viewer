# Cinematic 3D World Map — implementation plan (draft)

## Goal
Add a **2D / 3D toggle** to Aurora's existing World Map, retaining the current 2D map as the default. Aim for polished, fluid interactions comparable in *experience* to Google Earth and NASA Worldview without using their proprietary imagery, branding, code or copied UI layouts.

## Existing Aurora integration
- `crates/aurora-viewer/src/ui/worldmap/mod.rs`: existing world map, position tracking, teleport and overlays.
- `crates/aurora-viewer/src/ui/worldmap/overlay.rs`: search, region and landmark controls, history drawer.
- `crates/aurora-viewer/src/ui/map_tiles.rs`: map tile cache / imagery.
- `crates/aurora-viewer/src/world/worldmap.rs`: region metadata and status.
- `crates/aurora-viewer/src/world/terrain.rs`: terrain information for currently accessible regions.
- `crates/aurora-render`: rendering integration; should own new GPU 3D scene rather than a second independent renderer.

## Interaction contract
1. 2D is the default and must remain feature-complete, including search, landmarks, tracking, parcel boundaries where available, and teleport actions.
2. 3D toggle is shown **only when an actual 3D renderer exists** (no deceptive empty toggle). In 3D support pan, orbit, pitch, smooth fly-to, inertia, altitude/distance control, camera home/reset, and animated switch back to 2D.
3. Render a region map tile on top of a low-poly or high-resolution height mesh where terrain samples exist. Else use a flat textured region with a clear data-availability indicator; never manufacture exact terrain or building geometry.
4. Use distance-based level of detail and tile cache reuse; avoid loading an entire grid's objects at once. Existing map interactions and permission/RLVa restrictions remain authoritative.
5. Scene geometry from actual grid object data, if permitted and available, is distinct from approximate photogrammetry reconstructions. Do not label approximate reconstructions as exact.
6. Add optional time-based layers / visit-history browsing, like the timeline/animations in NASA Worldview, *only* from locally stored or explicitly authorized grid data. A normal visit-history entry is not proof that corresponding image/mesh data exists.

## Optional street-level / photo history mode
- Call it **Street-level preview** or **Visit View**, not Google Street View.
- Separate explicit settings for: enable photo-based reconstruction; keep generated local reconstruction cache. Both default to false and neither follows from turning 3D on.
- Never capture screenshots, read personal photo folders, scan historical snapshots, upload images, or create/share geometry without explicit authorization. No silent background indexing.
- Offer a per-session selection of existing user-owned photos and optional metadata with provenance; warn about usernames, visible residents, private parcels and other personal imagery.
- Reconstruct only if sufficient overlapping viewpoints permit it. A single picture can be used as a flat photo viewpoint but **cannot prove a true 3D scene**.
- Local-only processing by default; cloud processing, sharing, collaboration and public map contributions require further separate opt-ins.
- Expose preview, source, delete source links, clear generated cache, and disable controls. Respect grid-side rights and permission/privacy rules; never treat a photograph as permission to recreate restricted content.
- Provide a non-photogrammetry street-level fallback: an in-world camera position anchored to visited coordinates, when the viewer is online and authorized.

## Delivery stages
- **Stage 0 (started):** persisted schema for projection and separate photo reconstruction/cache consent, secure defaults.
- Stage 1: real 3D renderer + working toggle, camera controls, simple texture-on-height-mesh from available terrain, gracefully handling missing data.
- Stage 2: animation polish, map markers and teleport parity, dynamic region loading, quality presets, GPU/CPU budget tests.
- Stage 3: optional local Visit View, explicit photo import, provenance, deletion, privacy controls, graceful flat-photo mode.
- Stage 4: optional validated photogrammetry, time slider and revisits, cross-platform validation.

## Validation
- Schema defaults and backwards-compatible deserialization tests.
- 2D regression and same teleport/search behavior in 2D and 3D.
- GPU recovery, window detach, mixed-DPI, fullscreen and multi-monitor tests.
- Switching modes while tiles are loading, missing height data, region boundaries, stale cache.
- Privacy tests: no history-photo read or reconstruction without consent; disabling and deletion clear artifacts; no network requests from local reconstruction.
- Manual Windows/Linux/macOS smoke before offering for upstream review.

**Status:** The current branch contains only Stage 0 schema changes; 3D rendering, UI toggle and reconstruction are not implemented or validated. Do not advertise them as working features.
