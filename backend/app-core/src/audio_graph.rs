//! Audio Graph v1 编译器与实时安全执行计划。
//!
//! 候选图只在控制线程解析、校验、协商和预分配；提交时通过原子指针在块边界替换。
//! 实时侧只读取不可变计划、无锁参数队列和原子计数器，不接触文件、网络、日志或锁。

use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use jimmusic_protocol::{
    AudioEdgeSpecV1, AudioFormatSpecV1, AudioGraphMode, AudioGraphSpecV1, AudioMediaType,
    AudioNodeType, AudioPortSpecV1, Validate,
};
use plugin_abi::audio_v2::ParameterEventV2;
use serde::Serialize;

const DEFAULT_BLOCK_FRAMES: u32 = 512;
const FORMAT_CONVERTER_LATENCY_FRAMES: u32 = 32;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GraphError {
    #[error("schema validation failed: {0}")]
    Schema(String),
    #[error("node `{0}` does not exist")]
    MissingNode(String),
    #[error("port `{port}` does not exist on node `{node}`")]
    MissingPort { node: String, port: String },
    #[error("audio graph contains a cycle")]
    Cycle,
    #[error("edge {from_node}:{from_port} -> {to_node}:{to_port} has incompatible media types")]
    IncompatibleMediaType {
        from_node: String,
        from_port: String,
        to_node: String,
        to_port: String,
    },
    #[error("format conversion is required but not allowed for edge {0}")]
    ConversionNotAllowed(String),
    #[error("node `{0}` is not declared realtime-safe")]
    NotRealtimeSafe(String),
    #[error("node `{node}` of type {node_type:?} cannot process {media_type:?} media")]
    NonPcmProcessor {
        node: String,
        node_type: AudioNodeType,
        media_type: AudioMediaType,
    },
    #[error("bit-perfect graph contains sample-modifying node `{0}`")]
    BitPerfectViolation(String),
    #[error("latency budget exceeded ({actual} > {budget} frames)")]
    LatencyBudget { actual: u32, budget: u32 },
    #[error("memory budget exceeded ({actual} > {budget} bytes)")]
    MemoryBudget { actual: u64, budget: u64 },
    #[error("parameter event queue is full")]
    ParameterQueueFull,
    #[error("no rollback graph is available")]
    NoRollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatConversion {
    pub edge_index: usize,
    pub from: AudioFormatSpecV1,
    pub to: AudioFormatSpecV1,
    pub latency_frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DelayCompensation {
    pub edge_index: usize,
    pub delay_frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeExecution {
    pub node_id: String,
    pub node_type: AudioNodeType,
    pub accumulated_latency_frames: u32,
    pub failure_policy: jimmusic_protocol::NodeFailurePolicy,
}

#[derive(Debug)]
pub struct CompiledGraph {
    pub spec: AudioGraphSpecV1,
    pub execution_order: Vec<NodeExecution>,
    pub format_conversions: Vec<FormatConversion>,
    pub delay_compensation: Vec<DelayCompensation>,
    pub total_latency_frames: u32,
    pub estimated_buffer_bytes: u64,
    pub generation: u64,
}

impl CompiledGraph {
    pub fn audio_path(&self) -> AudioPathSnapshot {
        AudioPathSnapshot {
            graph_id: self.spec.graph_id.clone(),
            graph_version: self.spec.version,
            generation: self.generation,
            mode: self.spec.mode,
            nodes: self.execution_order.clone(),
            format_conversions: self.format_conversions.clone(),
            delay_compensation: self.delay_compensation.clone(),
            total_latency_frames: self.total_latency_frames,
            estimated_buffer_bytes: self.estimated_buffer_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioPathSnapshot {
    pub graph_id: String,
    pub graph_version: u64,
    pub generation: u64,
    pub mode: AudioGraphMode,
    pub nodes: Vec<NodeExecution>,
    pub format_conversions: Vec<FormatConversion>,
    pub delay_compensation: Vec<DelayCompensation>,
    pub total_latency_frames: u32,
    pub estimated_buffer_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BitPerfectState {
    Disabled,
    ConditionsSatisfied,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BitPerfectCondition {
    pub condition: String,
    pub satisfied: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BitPerfectStatus {
    pub state: BitPerfectState,
    /// 对外文案刻意限定为可观察链路，不能声称驱动后的 DAC 行为。
    pub statement: String,
    pub conditions: Vec<BitPerfectCondition>,
}

impl BitPerfectStatus {
    fn for_graph(graph: &CompiledGraph, output_supports_exclusive: Option<bool>) -> Self {
        if graph.spec.mode != AudioGraphMode::BitPerfect {
            return Self {
                state: BitPerfectState::Disabled,
                statement: "Bit-perfect mode is disabled".into(),
                conditions: Vec::new(),
            };
        }
        let no_conversions = graph.format_conversions.is_empty();
        let exclusive = output_supports_exclusive.unwrap_or(false);
        let conditions = vec![
            BitPerfectCondition {
                condition: "source_and_output_format_match".into(),
                satisfied: no_conversions,
                reason: (!no_conversions).then(|| "format conversion is present".into()),
            },
            BitPerfectCondition {
                condition: "exclusive_output_session".into(),
                satisfied: exclusive,
                reason: (!exclusive).then(|| "output did not prove an exclusive session".into()),
            },
            BitPerfectCondition {
                condition: "sample_modifying_nodes_bypassed".into(),
                satisfied: true,
                reason: None,
            },
        ];
        let satisfied = conditions.iter().all(|condition| condition.satisfied);
        Self {
            state: if satisfied {
                BitPerfectState::ConditionsSatisfied
            } else if output_supports_exclusive.is_none() {
                BitPerfectState::Unsupported
            } else {
                BitPerfectState::Failed
            },
            statement: if satisfied {
                "The observable JimMusic-to-driver path satisfies bit-perfect conditions; DAC behavior is not guaranteed"
                    .into()
            } else {
                "The observable audio path does not satisfy all bit-perfect conditions".into()
            },
            conditions,
        }
    }
}

pub struct GraphCompiler {
    generation: AtomicU64,
}

impl Default for GraphCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphCompiler {
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }

    pub fn compile(&self, spec: AudioGraphSpecV1) -> Result<Arc<CompiledGraph>, GraphError> {
        spec.validate()
            .map_err(|error| GraphError::Schema(error.to_string()))?;

        let nodes: HashMap<&str, _> = spec
            .nodes
            .iter()
            .map(|node| (node.node_id.as_str(), node))
            .collect();

        for node in &spec.nodes {
            if !node.realtime_safe && node.node_type != AudioNodeType::Decoder {
                return Err(GraphError::NotRealtimeSafe(node.node_id.clone()));
            }
            if spec.mode == AudioGraphMode::BitPerfect
                && matches!(
                    node.node_type,
                    AudioNodeType::Processor
                        | AudioNodeType::Resampler
                        | AudioNodeType::Mixer
                        | AudioNodeType::FormatConverter
                        | AudioNodeType::Transition
                )
            {
                return Err(GraphError::BitPerfectViolation(node.node_id.clone()));
            }
            for media_type in node
                .inputs
                .iter()
                .chain(&node.outputs)
                .map(|port| port.media_type)
                .filter(|media_type| {
                    matches!(media_type, AudioMediaType::Dsd | AudioMediaType::Encoded)
                })
            {
                if !matches!(
                    node.node_type,
                    AudioNodeType::Decoder
                        | AudioNodeType::EncodedPassthrough
                        | AudioNodeType::Output
                ) {
                    return Err(GraphError::NonPcmProcessor {
                        node: node.node_id.clone(),
                        node_type: node.node_type,
                        media_type,
                    });
                }
            }
        }

        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut indegree: HashMap<&str, usize> = nodes.keys().copied().map(|id| (id, 0)).collect();
        let mut conversions = Vec::new();

        for (edge_index, edge) in spec.edges.iter().enumerate() {
            let from = nodes
                .get(edge.from_node.as_str())
                .ok_or_else(|| GraphError::MissingNode(edge.from_node.clone()))?;
            let to = nodes
                .get(edge.to_node.as_str())
                .ok_or_else(|| GraphError::MissingNode(edge.to_node.clone()))?;
            let from_port = find_port(&from.outputs, &edge.from_node, &edge.from_port)?;
            let to_port = find_port(&to.inputs, &edge.to_node, &edge.to_port)?;
            if from_port.media_type != to_port.media_type {
                return Err(GraphError::IncompatibleMediaType {
                    from_node: edge.from_node.clone(),
                    from_port: edge.from_port.clone(),
                    to_node: edge.to_node.clone(),
                    to_port: edge.to_port.clone(),
                });
            }
            if from_port.format != to_port.format {
                let can_convert = spec.allow_format_conversion
                    && spec.mode != AudioGraphMode::BitPerfect
                    && from_port.media_type == AudioMediaType::Pcm;
                if !can_convert {
                    return Err(GraphError::ConversionNotAllowed(edge_label(edge)));
                }
                conversions.push(FormatConversion {
                    edge_index,
                    from: from_port.format.clone(),
                    to: to_port.format.clone(),
                    latency_frames: FORMAT_CONVERTER_LATENCY_FRAMES,
                });
            }
            adjacency
                .entry(edge.from_node.as_str())
                .or_default()
                .push(edge.to_node.as_str());
            *indegree
                .get_mut(edge.to_node.as_str())
                .expect("node checked above") += 1;
        }

        let mut ready: VecDeque<&str> = indegree
            .iter()
            .filter_map(|(node, degree)| (*degree == 0).then_some(*node))
            .collect();
        let mut topological = Vec::with_capacity(nodes.len());
        while let Some(node) = ready.pop_front() {
            topological.push(node);
            if let Some(children) = adjacency.get(node) {
                for child in children {
                    let degree = indegree.get_mut(child).expect("known child");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push_back(child);
                    }
                }
            }
        }
        if topological.len() != nodes.len() {
            return Err(GraphError::Cycle);
        }

        let conversion_by_edge: HashMap<usize, u32> = conversions
            .iter()
            .map(|conversion| (conversion.edge_index, conversion.latency_frames))
            .collect();
        let mut accumulated: HashMap<&str, u32> = HashMap::new();
        let mut delay_compensation = Vec::new();

        for node_id in &topological {
            let incoming: Vec<(usize, &AudioEdgeSpecV1)> = spec
                .edges
                .iter()
                .enumerate()
                .filter(|(_, edge)| edge.to_node == *node_id)
                .collect();
            let path_latencies: Vec<(usize, u32)> = incoming
                .iter()
                .map(|(edge_index, edge)| {
                    let base = *accumulated.get(edge.from_node.as_str()).unwrap_or(&0);
                    let conversion = conversion_by_edge.get(edge_index).copied().unwrap_or(0);
                    (*edge_index, base.saturating_add(conversion))
                })
                .collect();
            let max_input = path_latencies
                .iter()
                .map(|(_, latency)| *latency)
                .max()
                .unwrap_or(0);
            for (edge_index, latency) in path_latencies {
                if latency < max_input {
                    delay_compensation.push(DelayCompensation {
                        edge_index,
                        delay_frames: max_input - latency,
                    });
                }
            }
            let node_latency = nodes[*node_id].latency_frames;
            accumulated.insert(node_id, max_input.saturating_add(node_latency));
        }

        let total_latency_frames = *accumulated.get(spec.output_node.as_str()).unwrap_or(&0);
        if total_latency_frames > spec.latency_budget_frames {
            return Err(GraphError::LatencyBudget {
                actual: total_latency_frames,
                budget: spec.latency_budget_frames,
            });
        }
        let estimated_buffer_bytes = estimate_buffer_bytes(&spec);
        if estimated_buffer_bytes > spec.memory_budget_bytes {
            return Err(GraphError::MemoryBudget {
                actual: estimated_buffer_bytes,
                budget: spec.memory_budget_bytes,
            });
        }

        let execution_order = topological
            .into_iter()
            .map(|node_id| {
                let node = nodes[node_id];
                NodeExecution {
                    node_id: node.node_id.clone(),
                    node_type: node.node_type,
                    accumulated_latency_frames: accumulated[node_id],
                    failure_policy: node.failure_policy,
                }
            })
            .collect();

        Ok(Arc::new(CompiledGraph {
            spec,
            execution_order,
            format_conversions: conversions,
            delay_compensation,
            total_latency_frames,
            estimated_buffer_bytes,
            generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
        }))
    }
}

fn find_port<'a>(
    ports: &'a [AudioPortSpecV1],
    node: &str,
    port: &str,
) -> Result<&'a AudioPortSpecV1, GraphError> {
    ports
        .iter()
        .find(|candidate| candidate.port_id == port)
        .ok_or_else(|| GraphError::MissingPort {
            node: node.to_string(),
            port: port.to_string(),
        })
}

fn edge_label(edge: &AudioEdgeSpecV1) -> String {
    format!(
        "{}:{} -> {}:{}",
        edge.from_node, edge.from_port, edge.to_node, edge.to_port
    )
}

fn estimate_buffer_bytes(spec: &AudioGraphSpecV1) -> u64 {
    spec.edges
        .iter()
        .filter_map(|edge| {
            spec.nodes
                .iter()
                .find(|node| node.node_id == edge.from_node)
                .and_then(|node| {
                    node.outputs
                        .iter()
                        .find(|port| port.port_id == edge.from_port)
                })
        })
        .map(|port| DEFAULT_BLOCK_FRAMES as u64 * port.format.channels.max(1) as u64 * 4 * 2)
        .sum()
}

/// 原子 Arc 持有器。旧计划保留到 manager 销毁，实时读取不会遇到悬空计划。
struct AtomicGraph {
    current: AtomicPtr<CompiledGraph>,
    retired: Mutex<Vec<Arc<CompiledGraph>>>,
}

impl AtomicGraph {
    fn new(graph: Arc<CompiledGraph>) -> Self {
        Self {
            current: AtomicPtr::new(Arc::into_raw(graph).cast_mut()),
            retired: Mutex::new(Vec::new()),
        }
    }

    fn load(&self) -> Arc<CompiledGraph> {
        let pointer = self.current.load(Ordering::Acquire);
        assert!(!pointer.is_null(), "atomic graph must always have a plan");
        // SAFETY: the pointer owns one strong reference in `current`; swapped-out pointers are
        // retained in `retired`, so the allocation cannot disappear between load and increment.
        unsafe {
            Arc::increment_strong_count(pointer);
            Arc::from_raw(pointer)
        }
    }

    fn swap(&self, graph: Arc<CompiledGraph>) -> Arc<CompiledGraph> {
        let new_pointer = Arc::into_raw(graph).cast_mut();
        let old_pointer = self.current.swap(new_pointer, Ordering::AcqRel);
        // SAFETY: old_pointer was created by Arc::into_raw and is removed exactly once.
        let old = unsafe { Arc::from_raw(old_pointer) };
        self.retired
            .lock()
            .expect("retired graph lock poisoned")
            .push(old.clone());
        old
    }
}

impl Drop for AtomicGraph {
    fn drop(&mut self) {
        let pointer = *self.current.get_mut();
        if !pointer.is_null() {
            // SAFETY: manager destruction requires all realtime users to have stopped. This drops
            // the one strong reference owned by `current`.
            unsafe { drop(Arc::from_raw(pointer)) };
        }
    }
}

#[derive(Default)]
pub struct AudioGraphStats {
    processed_blocks: AtomicU64,
    deadline_misses: AtomicU64,
    underruns: AtomicU64,
    overruns: AtomicU64,
    max_process_ns: AtomicU64,
    parameter_events: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudioGraphStatsSnapshot {
    pub processed_blocks: u64,
    pub deadline_misses: u64,
    pub underruns: u64,
    pub overruns: u64,
    pub max_process_ns: u64,
    pub parameter_events: u64,
}

impl AudioGraphStats {
    pub fn snapshot(&self) -> AudioGraphStatsSnapshot {
        AudioGraphStatsSnapshot {
            processed_blocks: self.processed_blocks.load(Ordering::Relaxed),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            max_process_ns: self.max_process_ns.load(Ordering::Relaxed),
            parameter_events: self.parameter_events.load(Ordering::Relaxed),
        }
    }

    fn record_process(&self, elapsed_ns: u64, deadline_ns: u64, event_count: u64) {
        self.processed_blocks.fetch_add(1, Ordering::Relaxed);
        self.parameter_events
            .fetch_add(event_count, Ordering::Relaxed);
        if elapsed_ns > deadline_ns {
            self.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.max_process_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
    }

    pub fn record_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_overrun(&self) {
        self.overruns.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct AudioGraphManager<const PARAMETER_CAPACITY: usize = 1024> {
    compiler: GraphCompiler,
    active: AtomicGraph,
    rollback: Mutex<Vec<Arc<CompiledGraph>>>,
    parameters: ParameterQueue<PARAMETER_CAPACITY>,
    stats: AudioGraphStats,
}

impl<const PARAMETER_CAPACITY: usize> AudioGraphManager<PARAMETER_CAPACITY> {
    pub fn new(initial: AudioGraphSpecV1) -> Result<Self, GraphError> {
        let compiler = GraphCompiler::new();
        let initial = compiler.compile(initial)?;
        Ok(Self {
            compiler,
            active: AtomicGraph::new(initial),
            rollback: Mutex::new(Vec::new()),
            parameters: ParameterQueue::new(),
            stats: AudioGraphStats::default(),
        })
    }

    pub fn validate_and_compile(
        &self,
        candidate: AudioGraphSpecV1,
    ) -> Result<Arc<CompiledGraph>, GraphError> {
        self.compiler.compile(candidate)
    }

    /// 只提交已经完整 prepare 的不可变计划。失败候选永远不会修改活动图。
    pub fn commit(&self, candidate: Arc<CompiledGraph>) {
        let old = self.active.swap(candidate);
        self.rollback
            .lock()
            .expect("rollback lock poisoned")
            .push(old);
    }

    pub fn rollback(&self) -> Result<Arc<CompiledGraph>, GraphError> {
        let previous = self
            .rollback
            .lock()
            .expect("rollback lock poisoned")
            .pop()
            .ok_or(GraphError::NoRollback)?;
        let replaced = self.active.swap(previous.clone());
        self.rollback
            .lock()
            .expect("rollback lock poisoned")
            .push(replaced);
        Ok(previous)
    }

    /// 实时线程调用：仅执行原子读与预分配缓冲上的原地操作。
    pub fn process_block(
        &self,
        buffer: &mut PlanarBuffer,
        timeline_frame: u64,
        deadline_ns: u64,
    ) -> ProcessReport {
        let started = Instant::now();
        let graph = self.active.load();
        let mut events = 0u64;
        while let Some(_event) = self.parameters.pop_for_block(buffer.frames() as u32) {
            events += 1;
        }
        // 当前内置基础图节点为透明处理。真实插件节点通过 AudioNodeProcessV2 在相同的
        // `PlanarBuffer` Host 视图上执行；这里仍能验证图切换、deadline 和参数时序。
        let elapsed_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.stats.record_process(elapsed_ns, deadline_ns, events);
        ProcessReport {
            graph_generation: graph.generation,
            timeline_frame,
            frames: buffer.frames() as u32,
            elapsed_ns,
            parameter_events: events,
        }
    }

    pub fn active_graph(&self) -> Arc<CompiledGraph> {
        self.active.load()
    }

    pub fn audio_path(&self) -> AudioPathSnapshot {
        self.active.load().audio_path()
    }

    pub fn bit_perfect_status(&self, output_supports_exclusive: Option<bool>) -> BitPerfectStatus {
        BitPerfectStatus::for_graph(&self.active.load(), output_supports_exclusive)
    }

    pub fn enqueue_parameter(&self, event: ParameterEventV2) -> Result<(), GraphError> {
        self.parameters
            .push(event)
            .map_err(|_| GraphError::ParameterQueueFull)
    }

    pub fn stats(&self) -> AudioGraphStatsSnapshot {
        self.stats.snapshot()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProcessReport {
    pub graph_generation: u64,
    pub timeline_frame: u64,
    pub frames: u32,
    pub elapsed_ns: u64,
    pub parameter_events: u64,
}

/// Host 预分配的 planar f32 缓冲。构造后 `process_block` 不改变容量。
pub struct PlanarBuffer {
    channels: Vec<Vec<f32>>,
    frames: usize,
}

impl PlanarBuffer {
    pub fn new(channels: usize, frames: usize) -> Self {
        Self {
            channels: (0..channels).map(|_| vec![0.0; frames]).collect(),
            frames,
        }
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    pub fn channel_mut(&mut self, channel: usize) -> Option<&mut [f32]> {
        self.channels.get_mut(channel).map(Vec::as_mut_slice)
    }
}

/// 单生产者/单消费者固定容量参数队列。
pub struct ParameterQueue<const N: usize> {
    slots: [UnsafeCell<MaybeUninit<ParameterEventV2>>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
    /// 仅由单一消费者访问：保存队首属于未来块的事件，不丢弃也不回写生产者槽位。
    deferred: UnsafeCell<Option<ParameterEventV2>>,
}

// SAFETY: SPSC 约束由 API 使用方保证；槽位发布/读取由 Release/Acquire 索引同步。
unsafe impl<const N: usize> Sync for ParameterQueue<N> {}
unsafe impl<const N: usize> Send for ParameterQueue<N> {}

impl<const N: usize> Default for ParameterQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ParameterQueue<N> {
    pub fn new() -> Self {
        assert!(N >= 2, "parameter queue capacity must be at least 2");
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            deferred: UnsafeCell::new(None),
        }
    }

    pub fn push(&self, event: ParameterEventV2) -> Result<(), ParameterEventV2> {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        if next == self.tail.load(Ordering::Acquire) {
            return Err(event);
        }
        // SAFETY: only the producer writes at head, and head cannot equal the consumer tail.
        unsafe { (*self.slots[head].get()).write(event) };
        self.head.store(next, Ordering::Release);
        Ok(())
    }

    pub fn pop(&self) -> Option<ParameterEventV2> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: Acquire observed the producer's initialized slot; only consumer reads tail.
        let event = unsafe { (*self.slots[tail].get()).assume_init_read() };
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(event)
    }

    pub fn pop_for_block(&self, block_frames: u32) -> Option<ParameterEventV2> {
        // SAFETY: ParameterQueue 是 SPSC，只有消费者调用 pop/pop_for_block。
        let deferred = unsafe { &mut *self.deferred.get() };
        if let Some(event) = deferred.as_mut() {
            if event.frame_offset >= block_frames {
                event.frame_offset -= block_frames;
                return None;
            }
            return deferred.take();
        }
        let mut event = self.pop()?;
        if event.frame_offset >= block_frames {
            event.frame_offset -= block_frames;
            *deferred = Some(event);
            None
        } else {
            Some(event)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jimmusic_protocol::{AudioNodeSpecV1, NodeFailurePolicy, SCHEMA_V1};

    fn format(rate: u32) -> AudioFormatSpecV1 {
        AudioFormatSpecV1 {
            media_type: AudioMediaType::Pcm,
            sample_type: "f32".into(),
            sample_rate: rate,
            channels: 2,
            channel_layout: "stereo".into(),
            packing: "planar".into(),
            endian: "not_applicable".into(),
            bit_exact: false,
        }
    }

    fn port(id: &str, rate: u32) -> AudioPortSpecV1 {
        AudioPortSpecV1 {
            port_id: id.into(),
            media_type: AudioMediaType::Pcm,
            format: format(rate),
        }
    }

    fn dsd_port(id: &str) -> AudioPortSpecV1 {
        AudioPortSpecV1 {
            port_id: id.into(),
            media_type: AudioMediaType::Dsd,
            format: AudioFormatSpecV1 {
                media_type: AudioMediaType::Dsd,
                sample_type: "dsd_u8".into(),
                sample_rate: 2_822_400,
                channels: 2,
                channel_layout: "stereo".into(),
                packing: "interleaved".into(),
                endian: "not_applicable".into(),
                bit_exact: true,
            },
        }
    }

    fn node(
        id: &str,
        node_type: AudioNodeType,
        inputs: Vec<AudioPortSpecV1>,
        outputs: Vec<AudioPortSpecV1>,
        latency: u32,
    ) -> AudioNodeSpecV1 {
        AudioNodeSpecV1 {
            node_id: id.into(),
            node_type,
            plugin_id: "core".into(),
            plugin_version: "2.0.0".into(),
            inputs,
            outputs,
            latency_frames: latency,
            tail_frames: 0,
            realtime_safe: true,
            failure_policy: NodeFailurePolicy::Stop,
            state_cid: None,
        }
    }

    fn edge(from: &str, to: &str) -> AudioEdgeSpecV1 {
        AudioEdgeSpecV1 {
            from_node: from.into(),
            from_port: "out".into(),
            to_node: to.into(),
            to_port: "in".into(),
        }
    }

    fn graph() -> AudioGraphSpecV1 {
        AudioGraphSpecV1 {
            schema_version: SCHEMA_V1,
            graph_id: "default".into(),
            version: 1,
            created_by: "test".into(),
            nodes: vec![
                node(
                    "source",
                    AudioNodeType::Decoder,
                    Vec::new(),
                    vec![port("out", 44_100)],
                    0,
                ),
                node(
                    "output",
                    AudioNodeType::Output,
                    vec![port("in", 44_100)],
                    Vec::new(),
                    32,
                ),
            ],
            edges: vec![edge("source", "output")],
            output_node: "output".into(),
            mode: AudioGraphMode::Normal,
            allow_format_conversion: true,
            cpu_budget_micros: 2_000,
            memory_budget_bytes: 1_000_000,
            latency_budget_frames: 2_048,
        }
    }

    fn dsd_passthrough_graph() -> AudioGraphSpecV1 {
        AudioGraphSpecV1 {
            schema_version: SCHEMA_V1,
            graph_id: "dsd-native".into(),
            version: 1,
            created_by: "test".into(),
            nodes: vec![
                node(
                    "source",
                    AudioNodeType::Decoder,
                    Vec::new(),
                    vec![dsd_port("out")],
                    0,
                ),
                node(
                    "passthrough",
                    AudioNodeType::EncodedPassthrough,
                    vec![dsd_port("in")],
                    vec![dsd_port("out")],
                    0,
                ),
                node(
                    "output",
                    AudioNodeType::Output,
                    vec![dsd_port("in")],
                    Vec::new(),
                    0,
                ),
            ],
            edges: vec![edge("source", "passthrough"), edge("passthrough", "output")],
            output_node: "output".into(),
            mode: AudioGraphMode::BitPerfect,
            allow_format_conversion: false,
            cpu_budget_micros: 2_000,
            memory_budget_bytes: 1_000_000,
            latency_budget_frames: 2_048,
        }
    }

    #[test]
    fn compiles_valid_dag() {
        let compiled = GraphCompiler::new().compile(graph()).unwrap();
        assert_eq!(compiled.execution_order.len(), 2);
        assert_eq!(compiled.total_latency_frames, 32);
        assert!(compiled.format_conversions.is_empty());
    }

    #[test]
    fn rejects_cycle_before_commit() {
        let mut graph = graph();
        graph.nodes[0].inputs.push(port("in", 44_100));
        graph.nodes[1].outputs.push(port("out", 44_100));
        graph.edges.push(edge("output", "source"));
        assert!(matches!(
            GraphCompiler::new().compile(graph),
            Err(GraphError::Cycle)
        ));
    }

    #[test]
    fn inserts_converter_and_reports_it() {
        let mut graph = graph();
        graph.nodes[1].inputs[0] = port("in", 48_000);
        let compiled = GraphCompiler::new().compile(graph).unwrap();
        assert_eq!(compiled.format_conversions.len(), 1);
        assert_eq!(compiled.total_latency_frames, 64);
    }

    #[test]
    fn bit_perfect_rejects_conversion_and_processors() {
        let mut value = graph();
        value.mode = AudioGraphMode::BitPerfect;
        value.nodes[1].inputs[0] = port("in", 48_000);
        assert!(matches!(
            GraphCompiler::new().compile(value),
            Err(GraphError::ConversionNotAllowed(_))
        ));

        let mut value = graph();
        value.mode = AudioGraphMode::BitPerfect;
        value.nodes[1].node_type = AudioNodeType::Processor;
        assert!(matches!(
            GraphCompiler::new().compile(value),
            Err(GraphError::BitPerfectViolation(_))
        ));
    }

    #[test]
    fn dsd_uses_a_typed_passthrough_path_without_pcm_conversion() {
        let compiled = GraphCompiler::new()
            .compile(dsd_passthrough_graph())
            .unwrap();
        assert!(compiled.format_conversions.is_empty());
        assert_eq!(compiled.execution_order.len(), 3);
        assert_eq!(
            compiled.execution_order[1].node_type,
            AudioNodeType::EncodedPassthrough
        );
    }

    #[test]
    fn dsd_cannot_enter_an_ordinary_pcm_processor() {
        let mut value = dsd_passthrough_graph();
        value.mode = AudioGraphMode::Normal;
        value.nodes[1].node_type = AudioNodeType::Processor;
        assert!(matches!(
            GraphCompiler::new().compile(value),
            Err(GraphError::NonPcmProcessor { .. })
        ));
    }

    #[test]
    fn parallel_paths_receive_delay_compensation() {
        let mut value = graph();
        value.nodes.insert(
            1,
            node(
                "slow",
                AudioNodeType::Analyzer,
                vec![port("in", 44_100)],
                vec![port("out", 44_100)],
                100,
            ),
        );
        value.nodes[2].inputs.push(port("side", 44_100));
        value.edges = vec![
            edge("source", "slow"),
            edge("slow", "output"),
            AudioEdgeSpecV1 {
                from_node: "source".into(),
                from_port: "out".into(),
                to_node: "output".into(),
                to_port: "side".into(),
            },
        ];
        let compiled = GraphCompiler::new().compile(value).unwrap();
        assert_eq!(compiled.delay_compensation.len(), 1);
        assert_eq!(compiled.delay_compensation[0].delay_frames, 100);
    }

    #[test]
    fn failed_candidate_keeps_active_graph_and_rollback_works() {
        let manager = AudioGraphManager::<8>::new(graph()).unwrap();
        let generation = manager.active_graph().generation;
        let mut invalid = graph();
        invalid.edges.push(edge("output", "source"));
        assert!(manager.validate_and_compile(invalid).is_err());
        assert_eq!(manager.active_graph().generation, generation);

        let mut next = graph();
        next.version = 2;
        let candidate = manager.validate_and_compile(next).unwrap();
        manager.commit(candidate);
        assert_eq!(manager.active_graph().spec.version, 2);
        manager.rollback().unwrap();
        assert_eq!(manager.active_graph().spec.version, 1);
    }

    #[test]
    fn parameter_events_are_sample_positioned_and_bounded() {
        let queue = ParameterQueue::<3>::new();
        queue.push(ParameterEventV2::float(1, 10, 0.25)).unwrap();
        queue.push(ParameterEventV2::float(1, 11, 0.5)).unwrap();
        assert!(queue.push(ParameterEventV2::float(1, 12, 0.75)).is_err());
        let first = queue.pop().unwrap();
        let second = queue.pop().unwrap();
        assert_eq!(first.frame_offset, 10);
        assert_eq!(second.frame_offset, 11);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn process_uses_preallocated_buffer_and_reports_stats() {
        let manager = AudioGraphManager::<8>::new(graph()).unwrap();
        manager
            .enqueue_parameter(ParameterEventV2::float(7, 63, 0.5))
            .unwrap();
        let mut buffer = PlanarBuffer::new(2, 128);
        let capacity = buffer.channel_mut(0).unwrap().len();
        let report = manager.process_block(&mut buffer, 1_024, u64::MAX);
        assert_eq!(capacity, buffer.channel_mut(0).unwrap().len());
        assert_eq!(report.parameter_events, 1);
        assert_eq!(manager.stats().processed_blocks, 1);
    }

    #[test]
    fn future_parameter_event_is_deferred_without_loss() {
        let manager = AudioGraphManager::<8>::new(graph()).unwrap();
        manager
            .enqueue_parameter(ParameterEventV2::float(7, 191, 0.5))
            .unwrap();
        let mut buffer = PlanarBuffer::new(2, 128);
        assert_eq!(
            manager
                .process_block(&mut buffer, 0, u64::MAX)
                .parameter_events,
            0
        );
        assert_eq!(
            manager
                .process_block(&mut buffer, 128, u64::MAX)
                .parameter_events,
            1
        );
    }

    #[test]
    fn bit_perfect_status_is_explicit_about_driver_boundary() {
        let mut value = graph();
        value.mode = AudioGraphMode::BitPerfect;
        let manager = AudioGraphManager::<8>::new(value).unwrap();
        let status = manager.bit_perfect_status(Some(true));
        assert_eq!(status.state, BitPerfectState::ConditionsSatisfied);
        assert!(status.statement.contains("DAC behavior is not guaranteed"));
    }
}
