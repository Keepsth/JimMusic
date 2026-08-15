//! Wasmtime-based community plugin sandbox.
//!
//! The linker intentionally exposes no WASI implementation, filesystem, sockets,
//! clocks, environment variables, or process APIs. A guest can cross the host
//! boundary only through `jimmusic_host::capability_call`, and every call must
//! present an opaque handle owned by the same plugin instance. Handles are
//! permission-scoped and can be revoked while an instance is still running.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use jimmusic_protocol::PluginPermission;
use wasmtime::{
    Caller, Config, Engine, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
};

const HOST_MODULE: &str = "jimmusic_host";
const CAPABILITY_CALL: &str = "capability_call";

pub const CAPABILITY_INVALID_HANDLE: i64 = -1;
pub const CAPABILITY_PERMISSION_DENIED: i64 = -2;
pub const CAPABILITY_UNKNOWN_OPERATION: i64 = -3;
pub const CAPABILITY_HOST_ERROR: i64 = -4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapabilityHandle(u64);

impl CapabilityHandle {
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn as_guest_i64(self) -> i64 {
        self.0 as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CapabilityOperation {
    MusicLibraryRead = 1,
    MusicLibraryWrite = 2,
    IpfsFetch = 3,
    IpfsPublish = 4,
    NetworkRequest = 5,
    IsolatedStorageRead = 6,
    IsolatedStorageWrite = 7,
    DiagnosticsWrite = 8,
}

impl CapabilityOperation {
    pub const fn required_permission(self) -> PluginPermission {
        match self {
            Self::MusicLibraryRead => PluginPermission::MusicLibraryRead,
            Self::MusicLibraryWrite => PluginPermission::MusicLibraryWrite,
            Self::IpfsFetch => PluginPermission::IpfsFetch,
            Self::IpfsPublish => PluginPermission::IpfsPublish,
            Self::NetworkRequest => PluginPermission::NetworkDomains,
            Self::IsolatedStorageRead | Self::IsolatedStorageWrite => {
                PluginPermission::IsolatedStorage
            }
            Self::DiagnosticsWrite => PluginPermission::Diagnostics,
        }
    }
}

impl TryFrom<i32> for CapabilityOperation {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::MusicLibraryRead),
            2 => Ok(Self::MusicLibraryWrite),
            3 => Ok(Self::IpfsFetch),
            4 => Ok(Self::IpfsPublish),
            5 => Ok(Self::NetworkRequest),
            6 => Ok(Self::IsolatedStorageRead),
            7 => Ok(Self::IsolatedStorageWrite),
            8 => Ok(Self::DiagnosticsWrite),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequest {
    pub plugin_id: String,
    pub permission: PluginPermission,
    pub operation: CapabilityOperation,
    /// A fixed-width scalar argument keeps the generic boundary allocation-free.
    /// Rich operations use Host-owned object handles in this field.
    pub argument: i64,
}

/// Application services implement this interface after independently checking
/// domain allowlists, object ownership, and business invariants.
pub trait CapabilityHost: Send + Sync + 'static {
    fn invoke(&self, request: CapabilityRequest) -> Result<i64, String>;
}

#[derive(Default)]
pub struct DenyAllCapabilityHost;

impl CapabilityHost for DenyAllCapabilityHost {
    fn invoke(&self, _request: CapabilityRequest) -> Result<i64, String> {
        Err("no application capability host is configured".into())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SandboxLimits {
    pub max_memory_bytes: usize,
    pub max_table_elements: usize,
    pub fuel_per_invocation: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024,
            max_table_elements: 10_000,
            fuel_per_invocation: 10_000_000,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WasmSandboxError {
    #[error("failed to configure Wasmtime: {0}")]
    Engine(String),
    #[error("failed to compile WASM plugin: {0}")]
    Compile(String),
    #[error("ambient import `{module}::{name}` is forbidden")]
    ForbiddenImport { module: String, name: String },
    #[error("failed to instantiate WASM plugin: {0}")]
    Instantiate(String),
    #[error("WASM export `{0}` is missing or has the wrong (i64)->i64 signature")]
    Export(String),
    #[error("WASM invocation failed: {0}")]
    Invocation(String),
    #[error("plugin was not granted permission {0:?}")]
    PermissionDenied(PluginPermission),
    #[error("WASM artifact is unavailable: {0}")]
    Artifact(String),
}

#[derive(Clone)]
struct Grant {
    owner: String,
    permission: PluginPermission,
}

struct CapabilityRegistry {
    next: AtomicU64,
    grants: Mutex<HashMap<u64, Grant>>,
    host: Arc<dyn CapabilityHost>,
}

impl CapabilityRegistry {
    fn new(host: Arc<dyn CapabilityHost>) -> Self {
        Self {
            next: AtomicU64::new(1),
            grants: Mutex::new(HashMap::new()),
            host,
        }
    }

    fn issue(&self, owner: &str, permission: PluginPermission) -> CapabilityHandle {
        let value = self.next.fetch_add(1, Ordering::Relaxed).max(1);
        self.grants
            .lock()
            .expect("capability lock poisoned")
            .insert(
                value,
                Grant {
                    owner: owner.into(),
                    permission,
                },
            );
        CapabilityHandle(value)
    }

    fn revoke(&self, owner: &str, permission: PluginPermission) {
        self.grants
            .lock()
            .expect("capability lock poisoned")
            .retain(|_, grant| grant.owner != owner || grant.permission != permission);
    }

    fn revoke_all(&self, owner: &str) {
        self.grants
            .lock()
            .expect("capability lock poisoned")
            .retain(|_, grant| grant.owner != owner);
    }

    fn invoke(&self, owner: &str, handle: i64, operation: i32, argument: i64) -> i64 {
        if handle <= 0 {
            return CAPABILITY_INVALID_HANDLE;
        }
        let Ok(operation) = CapabilityOperation::try_from(operation) else {
            return CAPABILITY_UNKNOWN_OPERATION;
        };
        let grant = self
            .grants
            .lock()
            .expect("capability lock poisoned")
            .get(&(handle as u64))
            .cloned();
        let Some(grant) = grant else {
            return CAPABILITY_INVALID_HANDLE;
        };
        if grant.owner != owner || grant.permission != operation.required_permission() {
            return CAPABILITY_PERMISSION_DENIED;
        }
        self.host
            .invoke(CapabilityRequest {
                plugin_id: owner.into(),
                permission: grant.permission,
                operation,
                argument,
            })
            .unwrap_or(CAPABILITY_HOST_ERROR)
    }
}

struct SandboxState {
    plugin_id: String,
    registry: Arc<CapabilityRegistry>,
    limits: StoreLimits,
}

/// Shared compiler and capability broker. This type does not install WASI.
pub struct WasmSandboxRuntime {
    engine: Engine,
    registry: Arc<CapabilityRegistry>,
}

impl WasmSandboxRuntime {
    pub fn new(host: Arc<dyn CapabilityHost>) -> Result<Self, WasmSandboxError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine =
            Engine::new(&config).map_err(|error| WasmSandboxError::Engine(error.to_string()))?;
        Ok(Self {
            engine,
            registry: Arc::new(CapabilityRegistry::new(host)),
        })
    }

    pub fn deny_all() -> Result<Self, WasmSandboxError> {
        Self::new(Arc::new(DenyAllCapabilityHost))
    }

    pub fn instantiate(
        &self,
        plugin_id: impl Into<String>,
        module_bytes: &[u8],
        granted_permissions: BTreeSet<PluginPermission>,
        limits: SandboxLimits,
    ) -> Result<WasmPluginInstance, WasmSandboxError> {
        let plugin_id = plugin_id.into();
        let module = Module::new(&self.engine, module_bytes)
            .map_err(|error| WasmSandboxError::Compile(error.to_string()))?;
        for import in module.imports() {
            if import.module() != HOST_MODULE || import.name() != CAPABILITY_CALL {
                return Err(WasmSandboxError::ForbiddenImport {
                    module: import.module().into(),
                    name: import.name().into(),
                });
            }
        }

        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes.max(64 * 1024))
            .table_elements(limits.max_table_elements.max(1))
            .instances(1)
            .memories(1)
            .tables(2)
            .trap_on_grow_failure(true)
            .build();
        let state = SandboxState {
            plugin_id: plugin_id.clone(),
            registry: self.registry.clone(),
            limits: store_limits,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(limits.fuel_per_invocation.max(1))
            .map_err(|error| WasmSandboxError::Engine(error.to_string()))?;

        let mut linker = Linker::new(&self.engine);
        linker
            .func_wrap(
                HOST_MODULE,
                CAPABILITY_CALL,
                |caller: Caller<'_, SandboxState>,
                 handle: i64,
                 operation: i32,
                 argument: i64|
                 -> i64 {
                    caller.data().registry.invoke(
                        &caller.data().plugin_id,
                        handle,
                        operation,
                        argument,
                    )
                },
            )
            .map_err(|error| WasmSandboxError::Instantiate(error.to_string()))?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|error| WasmSandboxError::Instantiate(error.to_string()))?;

        Ok(WasmPluginInstance {
            plugin_id,
            granted_permissions: Mutex::new(granted_permissions),
            registry: self.registry.clone(),
            store,
            instance,
            fuel_per_invocation: limits.fuel_per_invocation.max(1),
        })
    }
}

pub struct WasmPluginInstance {
    plugin_id: String,
    granted_permissions: Mutex<BTreeSet<PluginPermission>>,
    registry: Arc<CapabilityRegistry>,
    store: Store<SandboxState>,
    instance: Instance,
    fuel_per_invocation: u64,
}

impl WasmPluginInstance {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Issues an opaque handle only for a permission granted at installation/runtime.
    pub fn issue_handle(
        &self,
        permission: PluginPermission,
    ) -> Result<CapabilityHandle, WasmSandboxError> {
        if !self
            .granted_permissions
            .lock()
            .expect("permission lock poisoned")
            .contains(&permission)
        {
            return Err(WasmSandboxError::PermissionDenied(permission));
        }
        Ok(self.registry.issue(&self.plugin_id, permission))
    }

    /// Revocation invalidates all existing handles for the permission immediately.
    pub fn revoke_permission(&self, permission: PluginPermission) {
        self.granted_permissions
            .lock()
            .expect("permission lock poisoned")
            .remove(&permission);
        self.registry.revoke(&self.plugin_id, permission);
    }

    pub fn invoke_i64(&mut self, export: &str, argument: i64) -> Result<i64, WasmSandboxError> {
        self.store
            .set_fuel(self.fuel_per_invocation)
            .map_err(|error| WasmSandboxError::Invocation(error.to_string()))?;
        let function = self
            .instance
            .get_typed_func::<i64, i64>(&mut self.store, export)
            .map_err(|_| WasmSandboxError::Export(export.into()))?;
        function
            .call(&mut self.store, argument)
            .map_err(|error| WasmSandboxError::Invocation(error.to_string()))
    }
}

impl Drop for WasmPluginInstance {
    fn drop(&mut self) {
        self.registry.revoke_all(&self.plugin_id);
    }
}

/// Owns active WASM instances and connects lifecycle enable/disable/revocation
/// to the sandbox. Native and declarative artifacts are deliberately ignored.
pub struct WasmPluginSupervisor {
    runtime: WasmSandboxRuntime,
    instances: Mutex<HashMap<String, WasmPluginInstance>>,
}

impl WasmPluginSupervisor {
    pub fn deny_all() -> Result<Self, WasmSandboxError> {
        Ok(Self {
            runtime: WasmSandboxRuntime::deny_all()?,
            instances: Mutex::new(HashMap::new()),
        })
    }

    pub fn with_host(host: Arc<dyn CapabilityHost>) -> Result<Self, WasmSandboxError> {
        Ok(Self {
            runtime: WasmSandboxRuntime::new(host)?,
            instances: Mutex::new(HashMap::new()),
        })
    }

    /// Returns `true` when the active artifact is WASM and was instantiated.
    pub fn activate(&self, record: &crate::PluginRuntimeRecord) -> Result<bool, WasmSandboxError> {
        let version = record
            .active_version
            .as_ref()
            .and_then(|version| record.versions.get(version))
            .ok_or_else(|| WasmSandboxError::Artifact("active version is missing".into()))?;
        if version.artifact.runtime != jimmusic_protocol::PluginRuntime::Wasm {
            return Ok(false);
        }
        let bytes = std::fs::read(&version.install_path)
            .map_err(|error| WasmSandboxError::Artifact(error.to_string()))?;
        let instance = self.runtime.instantiate(
            record.plugin_id.clone(),
            &bytes,
            record.permissions_granted.clone(),
            SandboxLimits::default(),
        )?;
        self.instances
            .lock()
            .expect("WASM instance lock poisoned")
            .insert(record.plugin_id.clone(), instance);
        Ok(true)
    }

    pub fn deactivate(&self, plugin_id: &str) -> bool {
        self.instances
            .lock()
            .expect("WASM instance lock poisoned")
            .remove(plugin_id)
            .is_some()
    }

    pub fn revoke_permission(&self, plugin_id: &str, permission: PluginPermission) {
        if let Some(instance) = self
            .instances
            .lock()
            .expect("WASM instance lock poisoned")
            .get(plugin_id)
        {
            instance.revoke_permission(permission);
        }
    }

    pub fn is_active(&self, plugin_id: &str) -> bool {
        self.instances
            .lock()
            .expect("WASM instance lock poisoned")
            .contains_key(plugin_id)
    }

    pub fn issue_handle(
        &self,
        plugin_id: &str,
        permission: PluginPermission,
    ) -> Result<CapabilityHandle, WasmSandboxError> {
        self.instances
            .lock()
            .expect("WASM instance lock poisoned")
            .get(plugin_id)
            .ok_or_else(|| WasmSandboxError::Artifact("plugin is not active".into()))?
            .issue_handle(permission)
    }

    pub fn invoke_i64(
        &self,
        plugin_id: &str,
        export: &str,
        argument: i64,
    ) -> Result<i64, WasmSandboxError> {
        self.instances
            .lock()
            .expect("WASM instance lock poisoned")
            .get_mut(plugin_id)
            .ok_or_else(|| WasmSandboxError::Artifact("plugin is not active".into()))?
            .invoke_i64(export, argument)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHost;

    impl CapabilityHost for TestHost {
        fn invoke(&self, request: CapabilityRequest) -> Result<i64, String> {
            assert_eq!(request.plugin_id, "org.example.sandboxed");
            Ok(request.argument + 1)
        }
    }

    fn runtime() -> WasmSandboxRuntime {
        WasmSandboxRuntime::new(Arc::new(TestHost)).unwrap()
    }

    #[test]
    fn capability_handles_are_scoped_checked_and_immediately_revocable() {
        let wasm = br#"
            (module
              (import "jimmusic_host" "capability_call"
                (func $call (param i64 i32 i64) (result i64)))
              (func (export "read") (param $handle i64) (result i64)
                local.get $handle
                i32.const 1
                i64.const 41
                call $call)
              (func (export "network") (param $handle i64) (result i64)
                local.get $handle
                i32.const 5
                i64.const 41
                call $call))
        "#;
        let mut instance = runtime()
            .instantiate(
                "org.example.sandboxed",
                wasm,
                BTreeSet::from([PluginPermission::MusicLibraryRead]),
                SandboxLimits::default(),
            )
            .unwrap();
        let handle = instance
            .issue_handle(PluginPermission::MusicLibraryRead)
            .unwrap();
        assert_eq!(
            instance.invoke_i64("read", handle.as_guest_i64()).unwrap(),
            42
        );
        assert_eq!(
            instance
                .invoke_i64("read", handle.as_guest_i64() + 10_000)
                .unwrap(),
            CAPABILITY_INVALID_HANDLE
        );
        assert_eq!(
            instance
                .invoke_i64("network", handle.as_guest_i64())
                .unwrap(),
            CAPABILITY_PERMISSION_DENIED
        );
        assert!(matches!(
            instance.issue_handle(PluginPermission::NetworkDomains),
            Err(WasmSandboxError::PermissionDenied(
                PluginPermission::NetworkDomains
            ))
        ));
        instance.revoke_permission(PluginPermission::MusicLibraryRead);
        assert_eq!(
            instance.invoke_i64("read", handle.as_guest_i64()).unwrap(),
            CAPABILITY_INVALID_HANDLE
        );
    }

    #[test]
    fn wasi_filesystem_and_socket_imports_are_rejected_before_instantiation() {
        for wasm in [
            br#"(module
                (import "wasi_snapshot_preview1" "path_open"
                  (func (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32))))"#
                .as_slice(),
            br#"(module
                (import "wasi:sockets/tcp" "start-bind"
                  (func (param i32) (result i32))))"#
                .as_slice(),
        ] {
            assert!(matches!(
                runtime().instantiate(
                    "org.example.sandboxed",
                    wasm,
                    BTreeSet::new(),
                    SandboxLimits::default(),
                ),
                Err(WasmSandboxError::ForbiddenImport { .. })
            ));
        }
    }

    #[test]
    fn memory_and_cpu_are_bounded() {
        let oversized = br#"(module (memory 1024))"#;
        assert!(matches!(
            runtime().instantiate(
                "org.example.sandboxed",
                oversized,
                BTreeSet::new(),
                SandboxLimits {
                    max_memory_bytes: 1024 * 1024,
                    ..SandboxLimits::default()
                },
            ),
            Err(WasmSandboxError::Instantiate(_))
        ));

        let looping = br#"
            (module
              (func (export "run") (param i64) (result i64)
                (loop $forever
                  i64.const 1
                  drop
                  br $forever)
                i64.const 0))
        "#;
        let mut instance = runtime()
            .instantiate(
                "org.example.sandboxed",
                looping,
                BTreeSet::new(),
                SandboxLimits {
                    fuel_per_invocation: 1_000,
                    ..SandboxLimits::default()
                },
            )
            .unwrap();
        assert!(matches!(
            instance.invoke_i64("run", 0),
            Err(WasmSandboxError::Invocation(_))
        ));
    }
}
