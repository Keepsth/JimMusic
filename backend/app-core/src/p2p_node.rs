//! Embedded IPFS/libp2p node used by the native application.
//!
//! This transport is deliberately independent from the Kubo HTTP compatibility
//! client. Blocks are exchanged through Bitswap over authenticated TCP/QUIC
//! connections and providers are discovered through the embedded Kademlia DHT.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cid::Cid;
use rust_ipfs::builder::DefaultIpfsBuilder;
use rust_ipfs::{
    Block, DhtMode, Ipfs, IpfsPath, Keypair, Multiaddr, PeerId, Protocol, RepoProvider,
};
use serde::{Deserialize, Serialize};

const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Eq)]
pub struct EmbeddedNodeConfig {
    pub repository_path: PathBuf,
    pub identity_protobuf: Vec<u8>,
    pub listen_addresses: Vec<String>,
    pub bootstrap_addresses: Vec<String>,
    pub enable_mdns: bool,
}

/// Loads the stable libp2p identity used by an app-embedded node, or creates it
/// atomically with private file permissions. The protobuf bytes are returned to
/// the caller and must never be logged or serialized into diagnostics.
pub fn load_or_create_identity(path: &Path) -> Result<Vec<u8>, EmbeddedNodeError> {
    if path.exists() {
        let bytes = std::fs::read(path).map_err(|error| {
            EmbeddedNodeError::Identity(format!("cannot read persisted key: {error}"))
        })?;
        Keypair::from_protobuf_encoding(&bytes)
            .map_err(|error| EmbeddedNodeError::Identity(error.to_string()))?;
        return Ok(bytes);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            EmbeddedNodeError::Identity(format!("cannot create identity directory: {error}"))
        })?;
    }
    let keypair = Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|error| EmbeddedNodeError::Identity(error.to_string()))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        // A concurrent app component may have won the create race. Re-read and
        // validate that key instead of silently replacing it.
        if path.exists() {
            return load_or_create_identity(path);
        }
        return Err(EmbeddedNodeError::Identity(format!(
            "cannot persist node identity: {error}"
        )));
    }
    Ok(bytes)
}

impl EmbeddedNodeConfig {
    pub fn native(repository_path: impl Into<PathBuf>, identity_protobuf: Vec<u8>) -> Self {
        Self {
            repository_path: repository_path.into(),
            identity_protobuf,
            listen_addresses: vec![
                "/ip4/0.0.0.0/tcp/0".into(),
                "/ip4/0.0.0.0/tcp/0/ws".into(),
                "/ip4/0.0.0.0/udp/0/quic-v1".into(),
            ],
            bootstrap_addresses: Vec::new(),
            enable_mdns: true,
        }
    }

    pub fn loopback(repository_path: impl Into<PathBuf>, identity_protobuf: Vec<u8>) -> Self {
        Self {
            repository_path: repository_path.into(),
            identity_protobuf,
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".into()],
            bootstrap_addresses: Vec::new(),
            enable_mdns: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddedNodeStatus {
    pub peer_id: String,
    pub listen_addresses: Vec<String>,
    pub connected_peers: Vec<String>,
    pub transports: Vec<String>,
    pub routing_status: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedNodeError {
    #[error("invalid embedded-node identity: {0}")]
    Identity(String),
    #[error("invalid multiaddress `{address}`: {reason}")]
    Address { address: String, reason: String },
    #[error("invalid CID or IPFS path `{value}`: {reason}")]
    ContentAddress { value: String, reason: String },
    #[error("embedded IPFS node failed to start: {0}")]
    Startup(String),
    #[error("embedded IPFS operation failed: {0}")]
    Operation(String),
    #[error("embedded IPFS operation timed out")]
    Timeout,
    #[error("content exceeds the configured {limit} byte read limit")]
    TooLarge { limit: usize },
}

/// Cloneable handle to one running embedded node.
pub struct EmbeddedIpfsNode {
    ipfs: Ipfs,
    peer_id: PeerId,
    configured_bootstrappers: usize,
    mdns_enabled: bool,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
}

impl EmbeddedIpfsNode {
    pub async fn start(config: EmbeddedNodeConfig) -> Result<Self, EmbeddedNodeError> {
        if config.identity_protobuf.is_empty() {
            return Err(EmbeddedNodeError::Identity(
                "private key encoding is empty".into(),
            ));
        }
        if config.listen_addresses.is_empty() {
            return Err(EmbeddedNodeError::Startup(
                "at least one listening address is required".into(),
            ));
        }
        let keypair = Keypair::from_protobuf_encoding(&config.identity_protobuf)
            .map_err(|error| EmbeddedNodeError::Identity(error.to_string()))?;
        let peer_id = keypair.public().to_peer_id();
        let mut builder = DefaultIpfsBuilder::with_keypair(keypair)
            .map_err(|error| EmbeddedNodeError::Identity(error.to_string()))?
            .with_default()
            .enable_tcp()
            .enable_websocket()
            .enable_quic()
            .enable_dns()
            .set_path(&config.repository_path)
            .set_provider(RepoProvider::Pinned);
        if config.enable_mdns {
            builder = builder.with_mdns();
        }
        for address in &config.listen_addresses {
            builder = builder.add_listening_addr(parse_multiaddr(address)?);
        }
        for address in &config.bootstrap_addresses {
            builder = builder.add_bootstrap(parse_multiaddr(address)?);
        }

        let ipfs = builder
            .start()
            .await
            .map_err(|error| EmbeddedNodeError::Startup(error.to_string()))?;
        ipfs.dht_mode(DhtMode::Server)
            .await
            .map_err(|error| EmbeddedNodeError::Startup(error.to_string()))?;

        if !config.bootstrap_addresses.is_empty() {
            // The routing table remains usable even if the public bootstrap set is
            // temporarily unavailable. Direct peer dialing and mDNS still work.
            if let Err(error) = ipfs.bootstrap().await {
                tracing::warn!(%error, "embedded IPFS bootstrap did not complete");
            }
        }

        Ok(Self {
            ipfs,
            peer_id,
            configured_bootstrappers: config.bootstrap_addresses.len(),
            mdns_enabled: config.enable_mdns,
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
        })
    }

    pub fn peer_id(&self) -> String {
        self.peer_id.to_string()
    }

    pub async fn status(&self) -> Result<EmbeddedNodeStatus, EmbeddedNodeError> {
        let listen_addresses = self.dialable_listen_addresses().await?;
        let connected_peers = self
            .ipfs
            .connected()
            .await
            .map_err(operation)?
            .into_iter()
            .map(|peer| peer.to_string())
            .collect::<Vec<_>>();
        let mut transports = vec![
            "bitswap".into(),
            "kademlia".into(),
            "tcp+noise".into(),
            "websocket".into(),
            "quic-v1".into(),
        ];
        if self.mdns_enabled {
            transports.push("mdns".into());
        }
        Ok(EmbeddedNodeStatus {
            peer_id: self.peer_id(),
            listen_addresses,
            connected_peers,
            transports,
            routing_status: if self.configured_bootstrappers == 0 {
                "ready_local_dht".into()
            } else {
                "ready_bootstrapped_dht".into()
            },
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
        })
    }

    pub async fn dialable_listen_addresses(&self) -> Result<Vec<String>, EmbeddedNodeError> {
        let mut addresses = self.ipfs.listening_addresses().await.map_err(operation)?;
        for address in &mut addresses {
            if !matches!(address.iter().last(), Some(Protocol::P2p(_))) {
                address.push(Protocol::P2p(self.peer_id));
            }
        }
        addresses.sort();
        addresses.dedup();
        Ok(addresses
            .into_iter()
            .map(|address| address.to_string())
            .collect())
    }

    pub async fn connect(&self, address: &str) -> Result<(), EmbeddedNodeError> {
        let address = parse_multiaddr(address)?;
        tokio::time::timeout(NETWORK_TIMEOUT, self.ipfs.connect(address))
            .await
            .map_err(|_| EmbeddedNodeError::Timeout)?
            .map_err(operation)?;
        Ok(())
    }

    pub async fn disconnect(&self, peer_id: &str) -> Result<(), EmbeddedNodeError> {
        let peer_id = PeerId::from_str(peer_id).map_err(|error| EmbeddedNodeError::Address {
            address: peer_id.to_string(),
            reason: error.to_string(),
        })?;
        self.ipfs.disconnect(peer_id).await.map_err(operation)
    }

    /// Adds an already content-addressed raw or DAG block. `Block::new`
    /// verifies the multihash before any bytes reach the repository.
    pub async fn put_block(
        &self,
        expected_cid: &str,
        bytes: &[u8],
        pin: bool,
    ) -> Result<(), EmbeddedNodeError> {
        let cid = parse_cid(expected_cid)?;
        let block = Block::new(cid, bytes.to_vec()).map_err(operation)?;
        self.ipfs.put_block(&block).await.map_err(operation)?;
        if pin {
            self.ipfs
                .insert_pin(cid)
                .recursive()
                .local()
                .await
                .map_err(operation)?;
        }
        self.bytes_up
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        // Publishing can legitimately fail before a routing peer is known; the
        // block remains immediately available to directly connected peers.
        if let Err(error) = self.ipfs.provide(cid).await {
            tracing::debug!(%error, %cid, "provider record deferred until a routing peer is known");
        }
        Ok(())
    }

    pub async fn get_block(&self, cid: &str) -> Result<Vec<u8>, EmbeddedNodeError> {
        let cid = parse_cid(cid)?;
        let block = tokio::time::timeout(NETWORK_TIMEOUT, self.ipfs.get_block(cid))
            .await
            .map_err(|_| EmbeddedNodeError::Timeout)?
            .map_err(operation)?;
        block.verify().map_err(operation)?;
        let bytes = block.as_ref().to_vec();
        self.bytes_down
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes)
    }

    /// Creates a Kubo/Helia-compatible UnixFS v1 DAG and returns its root CID.
    pub async fn add_unixfs(&self, bytes: &[u8], pin: bool) -> Result<String, EmbeddedNodeError> {
        let path = self
            .ipfs
            .add_unixfs(bytes.to_vec())
            .pin(pin)
            .await
            .map_err(operation)?;
        self.bytes_up
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let cid = path
            .root()
            .cid()
            .copied()
            .ok_or_else(|| EmbeddedNodeError::ContentAddress {
                value: path.to_string(),
                reason: "UnixFS add result has no immutable CID root".into(),
            })?;
        if let Err(error) = self.ipfs.provide(cid).await {
            tracing::debug!(%error, %cid, "UnixFS provider record deferred");
        }
        Ok(cid.to_string())
    }

    pub async fn cat_unixfs(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, EmbeddedNodeError> {
        let path = IpfsPath::from_str(path).map_err(|error| EmbeddedNodeError::ContentAddress {
            value: path.to_string(),
            reason: error.to_string(),
        })?;
        let bytes = self
            .ipfs
            .cat_unixfs(path)
            .max_length(max_bytes)
            .timeout(NETWORK_TIMEOUT)
            .await
            .map_err(|error| {
                let message = error.to_string();
                if message.contains("exceeded max length") {
                    EmbeddedNodeError::TooLarge { limit: max_bytes }
                } else {
                    EmbeddedNodeError::Operation(message)
                }
            })?;
        self.bytes_down
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes.to_vec())
    }

    pub async fn resolve_ipns_cid(&self, name: &str) -> Result<String, EmbeddedNodeError> {
        let value = format!("/ipns/{name}");
        let path =
            IpfsPath::from_str(&value).map_err(|error| EmbeddedNodeError::ContentAddress {
                value: value.clone(),
                reason: error.to_string(),
            })?;
        let resolved = tokio::time::timeout(NETWORK_TIMEOUT, self.ipfs.resolve_ipns(&path, true))
            .await
            .map_err(|_| EmbeddedNodeError::Timeout)?
            .map_err(operation)?;
        resolved
            .root()
            .cid()
            .map(ToString::to_string)
            .ok_or_else(|| EmbeddedNodeError::ContentAddress {
                value,
                reason: "resolved path has no immutable CID root".into(),
            })
    }

    pub async fn pin(&self, cid: &str) -> Result<(), EmbeddedNodeError> {
        let cid = parse_cid(cid)?;
        self.ipfs
            .insert_pin(cid)
            .recursive()
            .await
            .map_err(operation)
    }

    pub async fn unpin(&self, cid: &str) -> Result<(), EmbeddedNodeError> {
        let cid = parse_cid(cid)?;
        self.ipfs
            .remove_pin(cid)
            .recursive()
            .await
            .map_err(operation)
    }

    pub async fn shutdown(&self) {
        self.ipfs.clone().exit_daemon().await;
    }

    /// Consumes the final application handle so the filesystem repository lock
    /// is released and the same repository can be reopened in this process.
    pub async fn shutdown_owned(self) {
        self.ipfs.exit_daemon().await;
    }
}

fn parse_multiaddr(value: &str) -> Result<Multiaddr, EmbeddedNodeError> {
    Multiaddr::from_str(value).map_err(|error| EmbeddedNodeError::Address {
        address: value.to_string(),
        reason: error.to_string(),
    })
}

fn parse_cid(value: &str) -> Result<Cid, EmbeddedNodeError> {
    Cid::from_str(value).map_err(|error| EmbeddedNodeError::ContentAddress {
        value: value.to_string(),
        reason: error.to_string(),
    })
}

fn operation(error: impl ToString) -> EmbeddedNodeError {
    EmbeddedNodeError::Operation(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jimmusic_protocol::cid_v1_for_bytes;

    fn identity() -> Vec<u8> {
        Keypair::generate_ed25519().to_protobuf_encoding().unwrap()
    }

    #[test]
    fn persisted_identity_is_stable_and_private() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node-key.pb");
        let first = load_or_create_identity(&path).unwrap();
        let second = load_or_create_identity(&path).unwrap();
        assert_eq!(first, second);
        let first_peer = Keypair::from_protobuf_encoding(&first)
            .unwrap()
            .public()
            .to_peer_id();
        let second_peer = Keypair::from_protobuf_encoding(&second)
            .unwrap()
            .public()
            .to_peer_id();
        assert_eq!(first_peer, second_peer);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_nodes_exchange_verified_blocks_and_unixfs_without_a_gateway() {
        let first_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let first =
            EmbeddedIpfsNode::start(EmbeddedNodeConfig::loopback(first_dir.path(), identity()))
                .await
                .unwrap();
        let second =
            EmbeddedIpfsNode::start(EmbeddedNodeConfig::loopback(second_dir.path(), identity()))
                .await
                .unwrap();

        let first_address = first.dialable_listen_addresses().await.unwrap().remove(0);
        second.connect(&first_address).await.unwrap();

        let raw = b"gateway-free bitswap payload";
        let raw_cid = cid_v1_for_bytes(crate::node_service::RAW_CODEC, raw);
        first.put_block(&raw_cid, raw, true).await.unwrap();
        assert_eq!(second.get_block(&raw_cid).await.unwrap(), raw);

        let unixfs = vec![0x5a; 600_000];
        let unixfs_cid = first.add_unixfs(&unixfs, true).await.unwrap();
        assert_eq!(
            second.cat_unixfs(&unixfs_cid, unixfs.len()).await.unwrap(),
            unixfs
        );

        let first_status = first.status().await.unwrap();
        let second_status = second.status().await.unwrap();
        assert!(first_status.transports.contains(&"bitswap".to_string()));
        assert!(second_status.connected_peers.contains(&first.peer_id()));
        assert!(second_status.bytes_down >= raw.len() as u64 + 600_000);

        first.shutdown().await;
        second.shutdown().await;
    }

    #[tokio::test]
    async fn wrong_cid_is_rejected_before_repository_commit() {
        let dir = tempfile::tempdir().unwrap();
        let node = EmbeddedIpfsNode::start(EmbeddedNodeConfig::loopback(dir.path(), identity()))
            .await
            .unwrap();
        let wrong = cid_v1_for_bytes(crate::node_service::RAW_CODEC, b"other");
        assert!(node.put_block(&wrong, b"payload", true).await.is_err());
        node.shutdown().await;
    }
}
