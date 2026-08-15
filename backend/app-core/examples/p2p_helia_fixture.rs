//! Short-lived native fixture consumed by the Helia interoperability test.

use std::io::BufRead;

use app_core::p2p_node::{EmbeddedIpfsNode, EmbeddedNodeConfig};
use rust_ipfs::Keypair;
use sha2::{Digest, Sha256};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repository = tempfile::tempdir()?;
    let identity = Keypair::generate_ed25519().to_protobuf_encoding()?;
    let mut config = EmbeddedNodeConfig::loopback(repository.path(), identity);
    config.listen_addresses = vec!["/ip4/127.0.0.1/tcp/0/ws".into()];
    let node = EmbeddedIpfsNode::start(config).await?;
    let payload = (0..600_000)
        .map(|index| (index as u32).wrapping_mul(2_654_435_761) as u8)
        .collect::<Vec<_>>();
    let cid = node.add_unixfs(&payload, true).await?;
    let address = node
        .dialable_listen_addresses()
        .await?
        .into_iter()
        .find(|address| address.contains("/ws/"))
        .ok_or("native node did not expose a WebSocket address")?;
    println!(
        "{}",
        serde_json::json!({
            "schema_version": 1,
            "address": address,
            "cid": cid,
            "byte_length": payload.len(),
            "sha256": hex::encode(Sha256::digest(&payload)),
        })
    );

    // The parent test closes stdin after it has fetched and verified the DAG.
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    node.shutdown().await;
    Ok(())
}
