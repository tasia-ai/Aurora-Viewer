# Extension SDK quickstart (v1 draft)

The upstream framework includes two generic crates:

- `aurora-extension-api`: versioned JSON request/response types, event/action enums and permission names.
- `aurora-extension-sdk`: `ExtensionHandler`, request validation and `export_extension!(HandlerType)` to export the guest `alloc` and `handle` ABI.

Production extensions are separate projects. This document describes only the generic developer interface; no product plugin is included in the framework contribution.

## Minimal test handler

```rust
use aurora_extension_sdk::{ExtensionAction, ExtensionEvent, ExtensionHandler};

#[derive(Default)]
struct MinimalFixture;

impl ExtensionHandler for MinimalFixture {
    fn on_event(&mut self, event: &ExtensionEvent) -> Vec<ExtensionAction> {
        match event {
            ExtensionEvent::Initialize { .. } => vec![ExtensionAction::ShowNotification {
                title: "Fixture ready".into(),
                body: "This is a generic API test fixture.".into(),
            }],
            _ => Vec::new(),
        }
    }
}

aurora_extension_sdk::export_extension!(MinimalFixture);
```

The host sends an `ExtensionRequest` JSON envelope and the SDK returns a correlated `ExtensionResponse`. The host still enforces the manifest's requested and user-granted permissions; SDK validation is not a grant. Each event is handled with fresh guest state in the current runtime prototype. Do not store account-specific or persistent data in guest globals.

## Compile and package

The repository includes a neutral offline fixture at `crates/aurora-extension-sdk/examples/demo_extension.rs` with its manifest beside it. It registers one host-rendered panel and, when its button is clicked, requests a native notification. It reads no chat or account data and has no network, filesystem or secret capability.

Install the Rust `wasm32-unknown-unknown` target if needed, then build the fixture using the workspace's default development profile:

```powershell
rustup target add wasm32-unknown-unknown
cargo build -p aurora-extension-sdk --example demo_extension --target wasm32-unknown-unknown
cargo run -p aurora-extension-cli --bin aurora-ext -- pack --manifest crates/aurora-extension-sdk/examples/demo_extension.manifest.json --module target/wasm32-unknown-unknown/debug/examples/demo_extension.wasm --output .scratch/aurora-sandbox-demo.ext
cargo run -p aurora-extension-cli --bin aurora-ext -- validate .scratch/aurora-sandbox-demo.ext --api-version 1.0.0
```

In an offline demo viewer (`AURORA_DEMO=1`), open Preferences › Extensions and install the generated `.ext`. Review the publisher claim, explicitly grant `ui.panel.register` and `ui.notification.show`, and enable the fixture. The panel text confirms initialization; its button exercises the panel-event round trip and a native notification. The fixture is a test aid, not a production extension.

The packer sorts archive members, writes fixed timestamps/modes, rejects unsafe assets, validates the generated archive before writing, and refuses to overwrite an existing package. The viewer manager is integrated locally with explicit permission review, install/update/enable/disable/uninstall, revocation, safe mode and quarantine. See `IMPLEMENTATION_STATUS.md` for current validation status.

Supported manifest permissions are `chat.nearby.read`, `ui.chat.annotate`, `ui.notification.show` and `ui.panel.register`. Network access, filesystem access, secrets, IM/group-chat capture and native libraries are not available in v1. The publisher field is self-asserted; the package hash is an integrity identifier, not a signature.

## Current limitations

The persistent manager and Preferences UI are implemented locally, and the generic demo fixture has been packaged, validated and executed through Wasmi with explicit grants. The viewer's native file picker and package-review screen have been exercised; the permission-grant/install/enable/panel/notification sequence remains unverified in the viewer UI. The latest full prescribed script remains red at the viewer test step (9,073 passed, 21 failed, 22 ignored); publisher authenticity and cross-platform CI are also outstanding. There is no public plugin store or production plugin in this contribution. See `IMPLEMENTATION_STATUS.md` before relying on a feature.
