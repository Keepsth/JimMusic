// JimMusic Web 音频输出 —— 运行时页面胶水（参考实现）。
//
// 把 Rust wasm（backend/plugins/web-audio 编译为 wasm32 的 web_audio.wasm）的输出，
// 经 SharedArrayBuffer 无锁环形缓冲 + AudioWorklet 接到浏览器扬声器。
//
// 完整接通链路（内存布局与 backend/plugins/web-audio/src/bindings.rs 及 audio_worklet.js
// 约定一致）：
//
//   ┌── Rust wasm（PCM 生产者）──┐        ┌── AudioWorklet（PCM 消费者）──┐
//   │  jimmusic_output_write()   │        │  process() 每渲染量子读样本     │
//   └───────────┬────────────────┘        └───────────┬────────────────────┘
//               │ 写 head                             │ 读 tail
//               v                                      v
//   ┌──────────────────── SharedArrayBuffer ──────────────────────┐
//   │ [head: Int32Array[0]] [tail: Int32Array[1]] [i16 data ...]   │
//   └──────────────────────────────────────────────────────────────┘
//              head/tail 经 Atomics 原子读写（无锁 SPSC）
//
// 用法（页面初始化时调用）：
//   const out = await initJimMusicWebAudio({ sampleRate: 48000, channels: 2, frames: 8192 });
//   // out.push(samples: Int16Array) —— Rust wasm 侧调用 write 的 JS 等价接口，供上层对接。
//
// ⚠️ 注意：
// 1. 启用 SharedArrayBuffer 要求服务端返回跨源隔离响应头：
//      Cross-Origin-Opener-Policy: same-origin
//      Cross-Origin-Embedder-Policy: require-corp
// 2. 本文件是「接通路径」的参考实现：完整运行时依赖 wasm-bindgen 生成的胶水
//    （web_audio.wasm 经 wasm-bindgen CLI 处理）与浏览器环境，当前仓库的 wasm32
//    输出 ABI 尚未接 SharedArrayBuffer 绑定（见 bindings.rs 的 WebAudioRing 留待接线）。

'use strict';

const DATA_OFFSET_BYTES = 8; // head + tail 各一个 i32

/**
 * 初始化 Web 音频输出：加载 wasm、创建 SharedArrayBuffer 与 AudioWorkletNode，并接通。
 * @param {Object} opts
 * @param {number} [opts.sampleRate=48000] 采样率
 * @param {number} [opts.channels=2]        声道数
 * @param {number} [opts.frames=8192]       环形缓冲帧数（数据区样本数向上取 2 的幂）
 * @param {string} [opts.wasmUrl]           web_audio.wasm 的 URL
 * @param {string} [opts.workletUrl]        audio_worklet.js 的 URL
 * @returns {Promise<{context: AudioContext, sab: SharedArrayBuffer, push: Function}>}
 */
async function initJimMusicWebAudio(opts = {}) {
  const sampleRate = opts.sampleRate ?? 48000;
  const channels = opts.channels ?? 2;
  const frames = opts.frames ?? 8192;

  // 1. 创建 SharedArrayBuffer：[head i32][tail i32][i16 数据区]
  const dataSamples = nextPow2(frames * channels);
  const sab = new SharedArrayBuffer(DATA_OFFSET_BYTES + dataSamples * 2);
  const headView = new Int32Array(sab, 0, 2); // [head, tail]
  const dataView = new Int16Array(sab, DATA_OFFSET_BYTES);
  const capacity = dataView.length;
  const mask = capacity - 1;

  if (typeof Atomics === 'undefined') {
    throw new Error('SharedArrayBuffer requires cross-origin isolation (COOP/COEP headers)');
  }

  // 2. 创建 AudioContext + AudioWorkletNode。
  const context = new (window.AudioContext || window.webkitAudioContext)({ sampleRate });
  await context.audioWorklet.addModule(opts.workletUrl ?? 'audio_worklet.js');
  const node = new AudioWorkletNode(context, 'jimmusic-audio-output', {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [channels],
  });
  node.port.postMessage({ type: 'sab', sab });
  node.connect(context.destination);

  // 3. (可选) 实例化 Rust wasm 并把 SAB 交给 wasm 侧生产者。
  //    完整接通时，wasm 侧 WebAudioRing「构造」接收该 SAB，output 插件的 write 即写入此缓冲。
  const wasm = await loadWasm(opts.wasmUrl);
  bindWasmRing(wasm, sab);

  // 4. 暴露「生产」接口：上层（或 Rust 桥）写入交错 i16 PCM，写入 head 并原子推进。
  const push = (samples) => {
    const n = Math.min(samples.length, dataSamples);
    const head = Atomics.load(headView, 0);
    for (let i = 0; i < n; i++) {
      dataView[(head + i) & mask] = samples[i];
    }
    Atomics.store(headView, 0, (head + n) >>> 0);
    return n;
  };

  return { context, sab, push };
}

/** 加载并实例化 wasm（返回实例的 exports；失败返回 null，不阻断纯 JS 消费路径）。 */
async function loadWasm(wasmUrl) {
  if (!wasmUrl) return null;
  try {
    const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl));
    return instance;
  } catch (e) {
    console.warn('web_audio wasm 加载失败（走纯 JS 消费路径）:', e);
    return null;
  }
}

/** 把 SharedArrayBuffer 交给 wasm 侧（留待 wasm-bindgen 胶水接线）。 */
function bindWasmRing(wasm, sab) {
  // 完整接通时，这里调用 wasm 导出的构造/绑定，将 sab 映射为 WebAudioRing。
  // 当前为占位：WebAudioRing（bindings.rs）的实例化由 wasm-bindgen 胶水在 JS 侧完成。
  void wasm;
  void sab;
}

function nextPow2(n) {
  let p = 1;
  while (p < n) p <<= 1;
  return p;
}

// 挂到全局，便于 Flutter Web 层经 js_interop 调用。
if (typeof window !== 'undefined') {
  window.initJimMusicWebAudio = initJimMusicWebAudio;
}