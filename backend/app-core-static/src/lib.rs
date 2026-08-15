//! Static-link packaging target for iOS.
//!
//! `app-core` remains an rlib/cdylib for ordinary development and desktop or
//! Android builds. This small target asks Cargo to merge it and its dependencies
//! into one archive only when the iOS release job explicitly selects this crate.

pub use app_core::*;

/// A link anchor gives Xcode one ordinary symbol while `-force_load` preserves
/// every `jimmusic_host_*` and `jimmusic_node_*` C ABI export from app-core.
#[no_mangle]
pub extern "C" fn jimmusic_static_bridge_anchor() -> usize {
    app_core::host::jimmusic_host_state as *const () as usize
        ^ app_core::host::jimmusic_node_start as *const () as usize
        ^ app_core::host::jimmusic_node_status as *const () as usize
}
