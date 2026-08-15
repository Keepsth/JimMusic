//! 无锁单生产者/单消费者（SPSC）环形缓冲（需求 3.3 Web 音频桥的核心数据结构）。
//!
//! Web 平台下，Rust（wasm）无法直接驱动扬声器，需经 **AudioWorklet** 消费 PCM。
//! wasm 与 AudioWorklet 工作线程之间通过 `SharedArrayBuffer` + 原子操作传递实时音频。
//! [`RingBuffer`] 即该通道的无锁实现：生产者（wasm 侧解码器）`write`，消费者
//! （AudioWorklet）`read`，二者通过单调递增的原子 `head`/`tail` 游标同步，
//! 无需互斥锁，满足实时低延迟要求。
//!
//! 缓冲按 2 的幂采样数（i16）分配，游标用 `wrapping_sub` 计算占用，天然处理回绕。
//! 满时 `write` 返回实际写入数（`< 请求数` 即背压），空时 `read` 返回 `0`。
//!
//! 该结构为纯 Rust、无外部依赖、`Send + Sync`，可在原生平台直接单测；在 wasm32
//! 目标下由 [`crate::bindings`] 映射到 `SharedArrayBuffer` 内存上。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 无锁 SPSC 环形缓冲（i16 交错 PCM 样本）。
pub struct RingBuffer {
    /// 缓冲样本数（2 的幂）。
    capacity: usize,
    /// 取模掩码（`capacity - 1`）。
    mask: usize,
    /// 数据区。
    buffer: Box<[UnsafeCell<i16>]>,
    /// 生产者写游标（单调递增）。
    head: AtomicUsize,
    /// 消费者读游标（单调递增）。
    tail: AtomicUsize,
}

// 安全性：head/tail 由原子操作同步；数据区按 SPSC 纪律访问（生产者只写 head 侧、
// 消费者只读 tail 侧，互不重叠），跨线程共享安全。
unsafe impl Sync for RingBuffer {}
unsafe impl Send for RingBuffer {}

impl RingBuffer {
    /// 以 `capacity_frames` 帧分配缓冲（内部向上取整为 2 的幂样本数）。
    ///
    /// 注意：`capacity` 以**样本数**计；帧数 = 样本数 / 声道数。
    pub fn new(capacity_samples: usize) -> Self {
        let capacity = capacity_samples.max(2).next_power_of_two();
        let buffer = (0..capacity)
            .map(|_| UnsafeCell::new(0i16))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            capacity,
            mask: capacity - 1,
            buffer,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// 缓冲容量（i16 样本数）。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前可读样本数（已写入未读取）。
    pub fn available_read(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// 当前可写样本数（剩余空间）。
    pub fn available_write(&self) -> usize {
        self.capacity - self.available_read()
    }

    /// 是否为空（无未读样本）。
    pub fn is_empty(&self) -> bool {
        self.available_read() == 0
    }

    /// 生产者写入交错 PCM 样本，返回实际写入数（`< samples.len()` 表示背压）。
    pub fn write(&self, samples: &[i16]) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let space = self.capacity - head.wrapping_sub(tail);
        let n = samples.len().min(space);

        for (i, sample) in samples[..n].iter().enumerate() {
            let idx = (head + i) & self.mask;
            // SAFETY: SPSC 纪律保证该槽位不被消费者同时访问。
            unsafe { *self.buffer[idx].get() = *sample };
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    /// 消费者读取交错 PCM 样本到 `out`，返回实际读取数（`0` 表示空）。
    pub fn read(&self, out: &mut [i16]) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let avail = head.wrapping_sub(tail);
        let n = out.len().min(avail);

        for (i, slot) in out[..n].iter_mut().enumerate() {
            let idx = (tail + i) & self.mask;
            // SAFETY: SPSC 纪律保证该槽位不被生产者同时访问。
            *slot = unsafe { *self.buffer[idx].get() };
        }
        self.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
    }

    /// 丢弃未读样本（等价于读取后不保留）。
    pub fn clear(&self) {
        let head = self.head.load(Ordering::Acquire);
        self.tail.store(head, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn roundtrip_preserves_order() {
        let ring = RingBuffer::new(16);
        let data: Vec<i16> = (0..10).map(|i| i as i16).collect();
        assert_eq!(ring.write(&data), 10);
        assert_eq!(ring.available_read(), 10);

        let mut out = vec![0i16; 10];
        assert_eq!(ring.read(&mut out), 10);
        assert_eq!(out, data);
        assert!(ring.is_empty());
    }

    #[test]
    fn write_backpressures_when_full() {
        let ring = RingBuffer::new(8);
        let data: Vec<i16> = (0..20).map(|i| i as i16).collect();
        // 仅 8 个样本被接受（背压）。
        assert_eq!(ring.write(&data), 8);
        assert_eq!(ring.available_write(), 0);

        // 读走 4 个后，可再写 4 个。
        let mut out = vec![0i16; 4];
        assert_eq!(ring.read(&mut out), 4);
        assert_eq!(out, vec![0, 1, 2, 3]);
        assert_eq!(ring.available_write(), 4);
        assert_eq!(ring.write(&data[8..12]), 4);
    }

    #[test]
    fn wrap_around_reads_correctly() {
        // 容量 8，写入 8 再读 8，游标回绕后数据仍正确。
        let ring = RingBuffer::new(8);
        let a: Vec<i16> = (100..108).collect();
        assert_eq!(ring.write(&a), 8);
        let mut out = vec![0i16; 8];
        assert_eq!(ring.read(&mut out), 8);
        assert_eq!(out, a);

        // 第二轮：写读跨越掩码边界。
        let b: Vec<i16> = (200..212).collect();
        assert_eq!(ring.write(&b), 8);
        let mut out2 = vec![0i16; 8];
        assert_eq!(ring.read(&mut out2), 8);
        assert_eq!(out2, b[..8]);
    }

    #[test]
    fn clear_drops_unread() {
        let ring = RingBuffer::new(8);
        ring.write(&[1, 2, 3]);
        assert_eq!(ring.available_read(), 3);
        ring.clear();
        assert!(ring.is_empty());
    }

    #[test]
    fn spsc_threads_transfer_without_loss() {
        // 单生产者单消费者并发：验证无丢失、无越界。
        let ring = Arc::new(RingBuffer::new(1024));
        let total = 30_000usize;

        let producer = {
            let ring = ring.clone();
            thread::spawn(move || {
                let mut written = 0usize;
                while written < total {
                    let mut chunk = [0i16; 256];
                    for (i, s) in chunk.iter_mut().enumerate() {
                        *s = (written + i) as i16;
                    }
                    let n = ring.write(&chunk);
                    written += n;
                }
            })
        };

        let consumer = thread::spawn(move || {
            let mut read = 0usize;
            let mut next_expected = 0i16;
            let mut out = [0i16; 256];
            while read < total {
                let n = ring.read(&mut out);
                for &s in &out[..n] {
                    assert_eq!(s, next_expected, "sample out of order at {read}");
                    next_expected = next_expected.wrapping_add(1);
                }
                read += n;
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    }
}
