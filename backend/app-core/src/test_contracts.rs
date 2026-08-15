//! API-001：传输无关的服务契约断言。
//!
//! 同一断言集对多种传输适配器运行：
//! - FFI（`app-core/tests/node_ffi.rs`，直接调用 C ABI）；
//! - HTTP（`plugin-manager` 的 `/v1` 处理器测试）；
//! - JS/WASM 适配器通过 Helia 互操作测试验证 DAG 对象契约
//!   （`flutter_app/web_node/test/helia_native_interop.mjs`）。
//!
//! 该模块只服务于测试，不构成对外业务 API。

/// 节点状态契约：v1 schema、稳定非空 PeerId、生命周期状态、
/// 传输/监听矩阵与字节计数。
///
/// 返回 peer_id 供调用方继续断言跨重启稳定性。
pub fn assert_node_status_contract(value: &serde_json::Value) -> String {
    assert_eq!(value["schema_version"], 1, "{value}");
    let peer_id = value["peer_id"]
        .as_str()
        .filter(|peer_id| !peer_id.is_empty())
        .unwrap_or_else(|| panic!("peer_id must be present and non-empty: {value}"))
        .to_owned();
    assert!(
        !value["lifecycle_state"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "{value}"
    );
    assert!(
        value
            .get("transports")
            .is_some_and(serde_json::Value::is_array),
        "{value}"
    );
    assert!(
        value
            .get("connected_peers")
            .is_some_and(serde_json::Value::is_number),
        "{value}"
    );
    assert!(
        value
            .get("bytes_up")
            .is_some_and(serde_json::Value::is_number),
        "{value}"
    );
    assert!(
        value
            .get("bytes_down")
            .is_some_and(serde_json::Value::is_number),
        "{value}"
    );
    assert!(
        value
            .get("limitations")
            .is_some_and(serde_json::Value::is_array),
        "{value}"
    );
    peer_id
}

/// 健康端点契约：v1 schema 与状态字段。
pub fn assert_health_contract(value: &serde_json::Value) {
    assert_eq!(value["schema_version"], 1, "{value}");
    assert!(
        !value["status"].as_str().unwrap_or_default().is_empty(),
        "{value}"
    );
}
