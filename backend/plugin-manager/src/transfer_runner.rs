//! 持久传输任务执行器：有界并发、流式落盘、暂停/恢复、CID 校验与原子目标提交。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::node_service::RAW_CODEC;
use futures::StreamExt;
use jimmusic_protocol::{
    cid_v1_for_sha256_digest, ErrorEnvelopeV1, TransferKind, TransferState, DAG_CBOR_CODEC,
    SCHEMA_V1,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::state::AppState;

pub fn spawn(state: Arc<AppState>, task_id: String) {
    tokio::spawn(async move {
        if let Err(error) = run(state.clone(), &task_id).await {
            let still_owned = state.transfers.get(&task_id).is_some_and(|task| {
                matches!(
                    task.state,
                    TransferState::Resolving | TransferState::Transferring
                )
            });
            if still_owned {
                tracing::warn!(task_id, error = %error, "transfer task failed");
                let _ = fail(&state, &task_id, "transfer_failed", error, true);
            } else {
                // 状态已被控制面改变（重新排队/暂停/取消）：正常中止，不误报失败。
                tracing::info!(task_id, %error, "transfer run aborted after state change");
            }
        }
    });
}

async fn run(state: Arc<AppState>, task_id: &str) -> Result<(), String> {
    let (_permit, task) = loop {
        let scheduler = state.transfer_scheduler.lock().await;
        let task = state
            .transfers
            .get(task_id)
            .ok_or_else(|| format!("task `{task_id}` disappeared"))?;
        if task.state != TransferState::Queued {
            return Ok(());
        }
        let selected = state
            .transfers
            .next_queued()
            .ok_or_else(|| "transfer scheduler has no queued task".to_string())?;
        if selected.task_id != task_id {
            drop(scheduler);
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        let slots = state.transfer_slots.read().await.clone();
        let Ok(permit) = slots.try_acquire_owned() else {
            drop(scheduler);
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        };
        // NOD-006：任务排队执行前再次确认当前网络类别允许该策略；
        // 受限时由控制面暂停（结构化原因），本 runner 正常退出。
        let config = state.node.config();
        if state.transfers.network_restricted(
            &task.network_policy,
            config.network_class.as_deref(),
            config.metered_network_allowed,
        ) {
            let _ = state.transfers.apply_network_class(
                config.network_class.as_deref(),
                config.metered_network_allowed,
                app_core::transfer_service::now(),
            );
            return Ok(());
        }
        state
            .transfers
            .mark_resolving(task_id)
            .map_err(|error| error.to_string())?;
        drop(scheduler);
        break (permit, task);
    };

    if let Ok(bytes) = state.node.cat(&task.target_cid) {
        // 本地 CAS 命中：整块立即可用；写入 part 文件供流端点做完成交接。
        let _ = write_stream_part(&state, &task.task_id, &bytes).await;
        state
            .transfers
            .record_progress(
                task_id,
                bytes.len() as u64,
                Some(bytes.len() as u64),
                0,
                vec!["local-cas".into()],
            )
            .map_err(|error| error.to_string())?;
        let codec = detect_codec(&task.target_cid, Sha256::digest(&bytes).into())
            .ok_or_else(|| "local object CID codec is unsupported".to_string())?;
        state
            .transfers
            .verify_and_commit(
                task_id,
                codec,
                &bytes,
                &state.node,
                should_pin(task.kind),
                should_persist(task.kind, task.destination.as_deref()),
            )
            .map_err(|error| error.to_string())?;
        if let Some(destination) = task.destination {
            commit_destination_from_bytes(&destination, &bytes)
                .map_err(|error| error.to_string())?;
        }
        return Ok(());
    }

    if let Some(embedded) = state.embedded_node().await {
        if let Ok(bytes) = embedded.get_block(&task.target_cid).await {
            if bytes.len() as u64 > state.node.config().storage_limit_bytes {
                return Err("P2P block exceeds the configured storage limit".into());
            }
            // P2P 整块获取：块落地后写入 part 文件，边下边播客户端可随后
            // 立即开始播放并做完成交接（P2P 字节级渐进流仍是已知限制）。
            let _ = write_stream_part(&state, &task.task_id, &bytes).await;
            state
                .transfers
                .record_progress(
                    task_id,
                    bytes.len() as u64,
                    Some(bytes.len() as u64),
                    0,
                    vec!["embedded-bitswap".into()],
                )
                .map_err(|error| error.to_string())?;
            let codec = detect_codec(&task.target_cid, Sha256::digest(&bytes).into())
                .ok_or_else(|| "P2P block CID codec is unsupported".to_string())?;
            let pin = should_pin(task.kind);
            state
                .transfers
                .verify_and_commit(
                    task_id,
                    codec,
                    &bytes,
                    &state.node,
                    pin,
                    should_persist(task.kind, task.destination.as_deref()),
                )
                .map_err(|error| error.to_string())?;
            if pin {
                embedded
                    .put_block(&task.target_cid, &bytes, true)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if let Some(destination) = task.destination {
                commit_destination_from_bytes(&destination, &bytes)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
    }

    let transfer_dir = state.repo_dir.join("transfer-parts");
    tokio::fs::create_dir_all(&transfer_dir)
        .await
        .map_err(|error| error.to_string())?;
    let part_path = transfer_dir.join(format!("{task_id}.part"));
    let mut output = tokio::fs::File::create(&part_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut stream = state
        .ipfs
        .cat_stream(&task.target_cid)
        .await
        .map_err(|error| error.to_string())?;
    let max_bytes = state.node.config().storage_limit_bytes;
    let rate_limit = state.node.config().download_limit_bytes_per_second;
    let started = Instant::now();
    let mut completed = 0u64;
    let mut hasher = Sha256::new();

    while let Some(chunk) = stream.next().await {
        wait_if_paused(&state, task_id).await?;
        let chunk = chunk.map_err(|error| error.to_string())?;
        completed = completed.saturating_add(chunk.len() as u64);
        if completed > max_bytes {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(format!(
                "download exceeds storage limit of {max_bytes} bytes"
            ));
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        hasher.update(&chunk);
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let speed = (completed as f64 / elapsed) as u64;
        state
            .transfers
            .record_progress(
                task_id,
                completed,
                None,
                speed,
                vec!["configured-ipfs".into()],
            )
            .map_err(|error| error.to_string())?;
        // DST-010：蜂窝网络下计量每任务额度，超限时由服务暂停本任务，
        // runner 正常中止（状态已不是本 runner 所有，不会被误报失败）。
        if state.node.config().network_class.as_deref() == Some("cellular") {
            let updated = state
                .transfers
                .add_cellular_bytes(task_id, chunk.len() as u64)
                .map_err(|error| error.to_string())?;
            if updated.state == TransferState::Paused {
                return Err("cellular quota exceeded for this task".into());
            }
        }
        throttle(rate_limit, completed, started).await;
    }
    output.flush().await.map_err(|error| error.to_string())?;
    output.sync_all().await.map_err(|error| error.to_string())?;
    drop(output);

    let digest = hasher.finalize().into();
    let Some(codec) = detect_codec(&task.target_cid, digest) else {
        let _ = tokio::fs::remove_file(&part_path).await;
        let _ = fail(
            &state,
            task_id,
            "integrity_failed",
            "downloaded bytes do not match the requested raw or DAG-CBOR CID".into(),
            false,
        );
        return Ok(());
    };
    let transfers = state.transfers.clone();
    let node = state.node.clone();
    let task_id_owned = task_id.to_string();
    let verify_path = part_path.clone();
    let pin = should_pin(task.kind);
    let persistent = should_persist(task.kind, task.destination.as_deref());
    tokio::task::spawn_blocking(move || {
        transfers.verify_and_commit_file(
            &task_id_owned,
            codec,
            &verify_path,
            &node,
            pin,
            persistent,
        )
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;

    if let Some(destination) = task.destination {
        let source = part_path.clone();
        tokio::task::spawn_blocking(move || commit_destination(&destination, &source))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    }
    if pin {
        if let Some(embedded) = state.embedded_node().await {
            let bytes = tokio::fs::read(&part_path)
                .await
                .map_err(|error| error.to_string())?;
            embedded
                .put_block(&task.target_cid, &bytes, true)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    // 保留完整 part 文件：`/v1/transfers/{id}/stream` 在任务终结后仍可用它
    // 服务尾部字节（DST-007 边下边播完成交接）；孤儿文件由启动/流式清理。
    Ok(())
}

/// 把已完整获取的字节写入流端点 part 文件，供边下边播客户端播放、
/// Seek 与任务终结后的尾部交接（DST-007）。
async fn write_stream_part(state: &AppState, task_id: &str, bytes: &[u8]) -> std::io::Result<()> {
    let dir = state.repo_dir.join("transfer-parts");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{task_id}.part"));
    let mut file = tokio::fs::File::create(&path).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

async fn wait_if_paused(state: &AppState, task_id: &str) -> Result<(), String> {
    loop {
        let task = state
            .transfers
            .get(task_id)
            .ok_or_else(|| format!("task `{task_id}` disappeared"))?;
        match task.state {
            TransferState::Paused => tokio::time::sleep(Duration::from_millis(100)).await,
            TransferState::Cancelled => return Err("transfer was cancelled".into()),
            TransferState::Queued => {
                // 控制面重新排队（例如网络恢复后的自动恢复）：新 runner 接管，
                // 本 runner 中止且不把任务标记为失败。
                return Err("transfer was re-queued by the scheduler".into());
            }
            _ => return Ok(()),
        }
    }
}

async fn throttle(limit: Option<u64>, completed: u64, started: Instant) {
    let Some(limit) = limit.filter(|limit| *limit > 0) else {
        return;
    };
    let expected = Duration::from_secs_f64(completed as f64 / limit as f64);
    if expected > started.elapsed() {
        tokio::time::sleep(expected - started.elapsed()).await;
    }
}

fn detect_codec(expected: &str, digest: [u8; 32]) -> Option<u64> {
    [RAW_CODEC, DAG_CBOR_CODEC]
        .into_iter()
        .find(|codec| cid_v1_for_sha256_digest(*codec, digest) == expected)
}

fn should_pin(kind: TransferKind) -> bool {
    matches!(
        kind,
        TransferKind::Pin | TransferKind::Publish | TransferKind::Plugin
    )
}

fn should_persist(kind: TransferKind, destination: Option<&str>) -> bool {
    destination.is_some() || should_pin(kind) || kind == TransferKind::Download
}

fn commit_destination(destination: &str, source: &Path) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let target = PathBuf::from(destination);
    if target.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension("jimmusic.part");
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    std::fs::rename(temporary, target)
}

fn commit_destination_from_bytes(destination: &str, bytes: &[u8]) -> std::io::Result<()> {
    let temporary_dir = tempfile::tempdir()?;
    let source = temporary_dir.path().join("source");
    std::fs::write(&source, bytes)?;
    commit_destination(destination, &source)
}

fn fail(
    state: &AppState,
    task_id: &str,
    code: &str,
    message: String,
    retryable: bool,
) -> Result<(), String> {
    let terminal = if code == "integrity_failed" {
        TransferState::IntegrityFailed
    } else {
        TransferState::Failed
    };
    state
        .transfers
        .fail(
            task_id,
            terminal,
            ErrorEnvelopeV1 {
                schema_version: SCHEMA_V1,
                code: code.into(),
                message,
                subsystem: "transfer".into(),
                operation: "run".into(),
                retryable,
                unsupported_reason: None,
                details: BTreeMap::new(),
                request_id: None,
                causes: Vec::new(),
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_raw_and_dag_cbor_codecs_from_streaming_digest() {
        let digest: [u8; 32] = Sha256::digest(b"payload").into();
        let raw = cid_v1_for_sha256_digest(RAW_CODEC, digest);
        let dag = cid_v1_for_sha256_digest(DAG_CBOR_CODEC, digest);
        assert_eq!(detect_codec(&raw, digest), Some(RAW_CODEC));
        assert_eq!(detect_codec(&dag, digest), Some(DAG_CBOR_CODEC));
        assert_eq!(detect_codec("bafywrong", digest), None);
    }
}
