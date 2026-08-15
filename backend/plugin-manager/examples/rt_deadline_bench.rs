//! NFR-004：发布构建实时 deadline 基准。
//!
//! 在 release 构建下对编译后的音频图连续执行 `process_block`，
//! 统计相对块 deadline 的超时并原子记录基线 JSON：
//!
//! ```sh
//! cargo run --release --locked -p plugin-manager --example rt_deadline_bench \
//!   -- <iterations> <baseline-output.json>
//! ```
//!
//! 预算：超时占比 ≤ 1%（共享 CI 机器存在调度抖动；本地应接近 0）。
//! 超预算时以非零码退出，供 CI 门禁使用。

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use app_core::audio_graph::{AudioGraphManager, PlanarBuffer};
use jimmusic_protocol::{
    AudioEdgeSpecV1, AudioFormatSpecV1, AudioGraphMode, AudioGraphSpecV1, AudioMediaType,
    AudioNodeSpecV1, AudioNodeType, AudioPortSpecV1, NodeFailurePolicy, SCHEMA_V1,
};

const BLOCK_FRAMES: u32 = 512;
const SAMPLE_RATE: u32 = 48_000;

fn graph() -> AudioGraphSpecV1 {
    let format = AudioFormatSpecV1 {
        media_type: AudioMediaType::Pcm,
        sample_type: "f32".into(),
        sample_rate: SAMPLE_RATE,
        channels: 2,
        channel_layout: "stereo".into(),
        packing: "planar".into(),
        endian: "not_applicable".into(),
        bit_exact: false,
    };
    AudioGraphSpecV1 {
        schema_version: SCHEMA_V1,
        graph_id: "rt-deadline-bench".into(),
        version: 1,
        created_by: "rt_deadline_bench".into(),
        nodes: vec![
            AudioNodeSpecV1 {
                node_id: "source".into(),
                node_type: AudioNodeType::Decoder,
                plugin_id: "core".into(),
                plugin_version: "2.0.0".into(),
                inputs: Vec::new(),
                outputs: vec![AudioPortSpecV1 {
                    port_id: "out".into(),
                    media_type: AudioMediaType::Pcm,
                    format: format.clone(),
                }],
                latency_frames: 0,
                tail_frames: 0,
                realtime_safe: true,
                failure_policy: NodeFailurePolicy::Stop,
                state_cid: None,
            },
            AudioNodeSpecV1 {
                node_id: "output".into(),
                node_type: AudioNodeType::Output,
                plugin_id: "core".into(),
                plugin_version: "2.0.0".into(),
                inputs: vec![AudioPortSpecV1 {
                    port_id: "in".into(),
                    media_type: AudioMediaType::Pcm,
                    format,
                }],
                outputs: Vec::new(),
                latency_frames: 32,
                tail_frames: 0,
                realtime_safe: true,
                failure_policy: NodeFailurePolicy::Stop,
                state_cid: None,
            },
        ],
        edges: vec![AudioEdgeSpecV1 {
            from_node: "source".into(),
            from_port: "out".into(),
            to_node: "output".into(),
            to_port: "in".into(),
        }],
        output_node: "output".into(),
        mode: AudioGraphMode::Normal,
        allow_format_conversion: true,
        cpu_budget_micros: 2_000,
        memory_budget_bytes: 1_000_000,
        latency_budget_frames: 2_048,
    }
}

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let iterations: u64 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000);
    let output = args
        .next()
        .unwrap_or_else(|| "rt-deadline-baseline.json".into());

    let manager = AudioGraphManager::<8>::new(graph()).expect("graph must compile");
    let mut buffer = PlanarBuffer::new(2, BLOCK_FRAMES as usize);
    let deadline_ns = u64::from(BLOCK_FRAMES) * 1_000_000_000 / u64::from(SAMPLE_RATE);

    let mut misses = 0u64;
    let mut max_elapsed_ns = 0u64;
    let mut total_elapsed_ns = 0u128;
    let started = Instant::now();
    for frame in 0..iterations {
        let report = manager.process_block(&mut buffer, frame, deadline_ns);
        total_elapsed_ns += u128::from(report.elapsed_ns);
        max_elapsed_ns = max_elapsed_ns.max(report.elapsed_ns);
        if report.elapsed_ns > deadline_ns {
            misses += 1;
        }
    }
    let wall_ms = started.elapsed().as_millis() as u64;
    let recorded_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();

    let baseline = serde_json::json!({
        "schema_version": 1,
        "iterations": iterations,
        "block_frames": BLOCK_FRAMES,
        "sample_rate": SAMPLE_RATE,
        "deadline_ns": deadline_ns,
        "deadline_misses": misses,
        "max_elapsed_ns": max_elapsed_ns,
        "mean_elapsed_ns": (total_elapsed_ns / u128::from(iterations.max(1))) as u64,
        "wall_ms": wall_ms,
        "recorded_at": recorded_at,
    });
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(&baseline).expect("baseline JSON"),
    )
    .unwrap_or_else(|error| panic!("write baseline `{output}`: {error}"));
    println!("{}", serde_json::to_string(&baseline).unwrap());

    let budget = iterations / 100;
    if misses > budget {
        eprintln!(
            "deadline budget exceeded: {misses} misses > {budget} budget \
             ({iterations} iterations, deadline {deadline_ns} ns)"
        );
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
