use std::ffi::CString;

use app_core::host::{
    jimmusic_node_set_foreground, jimmusic_node_start, jimmusic_node_status, jimmusic_node_stop,
};

fn status() -> serde_json::Value {
    // SAFETY: a null buffer is the documented size query.
    let length = unsafe { jimmusic_node_status(std::ptr::null_mut(), 0) };
    let mut bytes = vec![0_u8; length];
    // SAFETY: `bytes` owns exactly `length` writable bytes.
    let written = unsafe { jimmusic_node_status(bytes.as_mut_ptr(), bytes.len()) };
    assert_eq!(written, length);
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn native_node_ffi_persists_identity_and_reports_lifecycle_honestly() {
    let directory = tempfile::tempdir().unwrap();
    let root = CString::new(directory.path().to_string_lossy().as_bytes()).unwrap();
    assert_eq!(jimmusic_node_stop(), 0);
    let initial_start = jimmusic_node_start(root.as_ptr());
    assert_eq!(initial_start, 0, "initial start failed: {}", status());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let foreground = loop {
        let snapshot = status();
        if snapshot["listen_addresses"]
            .as_array()
            .is_some_and(|addresses| addresses.len() >= 3)
            || std::time::Instant::now() >= deadline
        {
            break snapshot;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    assert_eq!(foreground["implementation"], "rust-ipfs");
    assert_eq!(foreground["lifecycle_state"], "foreground");
    assert_eq!(foreground["persists_after_app_close"], false);
    assert!(
        foreground["listen_addresses"].as_array().unwrap().len() >= 3,
        "status did not expose all configured transports: {foreground}"
    );
    assert!(foreground["transports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|transport| transport == "bitswap"));
    let peer_id = foreground["peer_id"].as_str().unwrap().to_owned();

    assert_eq!(jimmusic_node_set_foreground(0), 0);
    assert_eq!(status()["lifecycle_state"], "background_degraded");
    assert_eq!(jimmusic_node_stop(), 0);
    assert_eq!(status()["lifecycle_state"], "stopped");

    let restart = jimmusic_node_start(root.as_ptr());
    assert_eq!(restart, 0, "restart failed: {}", status());
    assert_eq!(status()["peer_id"], peer_id);
    assert_eq!(jimmusic_node_stop(), 0);
}
