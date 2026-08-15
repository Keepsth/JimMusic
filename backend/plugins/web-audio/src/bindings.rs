//! wasm-bindgen 桥：把 SharedArrayBuffer 映射为无锁环形缓冲（仅 wasm32 编译）。
//!
//! 内存布局（与 JS 侧 `audio_worklet.js` 约定一致）：
//! - `Int32Array[0]`：`head`（生产者写游标，Rust 经 Atomics 更新）；
//! - `Int32Array[1]`：`tail`（消费者读游标，AudioWorklet 经 Atomics 更新）；
//! - `Int16Array[byte offset 8]`：数据区（交错 PCM，容量为 2 的幂样本数）。
//!
//! Rust（wasm）作为 PCM 生产者调用 [`WebAudioRing::push`]；浏览器端 AudioWorklet
//! 处理器周期读取 `tail` 之前的数据并推进 `tail`。二者互不重叠，无需互斥锁。

use js_sys::{Atomics, Int16Array, Int32Array, SharedArrayBuffer};
use wasm_bindgen::prelude::*;

/// head + tail 各占一个 i32（共 8 字节），数据区从第 8 字节开始。
const HEAD_TAIL_BYTES: u32 = 8;

/// wasm 侧生产者句柄：持有 SharedArrayBuffer 上的环形缓冲视图。
#[wasm_bindgen]
pub struct WebAudioRing {
    head_view: Int32Array,
    tail_view: Int32Array,
    data_view: Int16Array,
    /// 数据区样本容量（2 的幂）。
    capacity: u32,
    mask: u32,
}

#[wasm_bindgen]
impl WebAudioRing {
    /// 从 SharedArrayBuffer 构造（约定：字节 0..8 为 head/tail，其后为 i16 数据区）。
    #[wasm_bindgen(constructor)]
    pub fn new(sab: &SharedArrayBuffer) -> WebAudioRing {
        let head_view = Int32Array::new_with_byte_offset_and_length(&sab.into(), 0, 2);
        let data_len = (sab.byte_length() / 2).saturating_sub(HEAD_TAIL_BYTES / 2);
        let data_view =
            Int16Array::new_with_byte_offset_and_length(&sab.into(), HEAD_TAIL_BYTES, data_len);
        let capacity = data_view.length().next_power_of_two();
        WebAudioRing {
            tail_view: head_view.clone(),
            head_view,
            data_view,
            capacity,
            mask: capacity - 1,
        }
    }

    /// 生产者写入交错 PCM，返回实际写入样本数（`< samples.len()` 表示背压）。
    pub fn push(&self, samples: &[i16]) -> usize {
        let head = Atomics::load(&self.head_view, 0).expect("atomics load head") as u32;
        let tail = Atomics::load(&self.tail_view, 1).expect("atomics load tail") as u32;
        let space = self.capacity.saturating_sub(head.wrapping_sub(tail));
        let n = (samples.len() as u32).min(space) as usize;

        for (i, &sample) in samples[..n].iter().enumerate() {
            let idx = (head + i as u32) & self.mask;
            self.data_view.set_index(idx, sample);
        }
        let new_head = head.wrapping_add(n as u32) as i32;
        Atomics::store(&self.head_view, 0, new_head).expect("atomics store head");
        n
    }

    /// 当前可写样本数（供生产者决定是否等待）。
    pub fn available_write(&self) -> u32 {
        let head = Atomics::load(&self.head_view, 0).expect("atomics load head") as u32;
        let tail = Atomics::load(&self.tail_view, 1).expect("atomics load tail") as u32;
        self.capacity.saturating_sub(head.wrapping_sub(tail))
    }
}
