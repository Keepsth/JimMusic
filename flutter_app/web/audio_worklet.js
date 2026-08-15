// JimMusic Web 音频桥 —— AudioWorklet 处理器（需求 3.3「Web 平台」）。
//
// 与 backend/plugins/web-audio 的 wasm-bindgen 桥配合：Rust（wasm）作为 PCM 生产者，
// 经 SharedArrayBuffer 环形缓冲写入交错 i16 样本；本处理器作为消费者，在每个渲染
// 量子（render quantum）内读取样本、转成 Float32 并输出到扬声器。
//
// 内存布局（与 web-audio/src/bindings.rs 约定一致）：
//   - Int32Array[0] : head（生产者写游标，Rust 经 Atomics 更新）
//   - Int32Array[1] : tail（消费者读游标，本处理器经 Atomics 更新）
//   - Int16Array[offset = 2 个 i32] : 数据区（交错 PCM，容量为 2 的幂样本数）
//
// 用法（页面初始化时）：
//   const sab = new SharedArrayBuffer(8 + DATA_SAMPLES * 2);
//   // 需要服务端返回 COOP/COEP 头以启用 SharedArrayBuffer：
//   //   Cross-Origin-Opener-Policy: same-origin
//   //   Cross-Origin-Embedder-Policy: require-corp
//   const moduleUrl = URL.createObjectURL(new Blob([workletSource], { type: 'text/javascript' }));
//   await audioCtx.audioWorklet.addModule(moduleUrl);
//   const node = new AudioWorkletNode(audioCtx, 'jimmusic-audio-output');
//   // 将 sab 传给 processor（经 port 或构造选项的 processorOptions）。
//   node.port.postMessage({ type: 'sab', sab });
//   node.connect(audioCtx.destination);
//   // 再将 sab 交给 wasm 侧 WebAudioRing（见 bindings.rs）。

class JimMusicAudioOutputProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.sab = null;
    this.headView = null;
    this.tailView = null;
    this.dataView = null;
    this.capacity = 0;
    this.mask = 0;
    this.port.onmessage = (e) => {
      if (e.data && e.data.type === 'sab') {
        this.init(e.data.sab);
      }
    };
  }

  init(sab) {
    this.sab = sab;
    this.headView = new Int32Array(sab, 0, 2);
    // 数据区从第 2 个 i32（第 8 字节）开始。
    this.dataView = new Int16Array(sab, 8);
    this.capacity = this.nextPow2(this.dataView.length);
    this.mask = this.capacity - 1;
  }

  nextPow2(n) {
    let p = 1;
    while (p < n) p <<= 1;
    return p;
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0 || !this.sab) {
      return true;
    }
    const channelCount = output.length;
    const frameCount = output[0].length;
    const samplesNeeded = frameCount * channelCount;

    // 可读样本数（head - tail）。
    const head = Atomics.load(this.headView, 0);
    const tail = Atomics.load(this.tailView, 1);
    let available = (head - tail) & 0xffffffff;
    const n = Math.min(samplesNeeded, available);

    // 读取交错样本并拆分到各声道（Float32）。
    for (let i = 0; i < n; i++) {
      const idx = this.mask & (tail + i);
      const v = this.dataView[idx] / 32768.0;
      output[i % channelCount][Math.floor(i / channelCount)] = v;
    }

    // 推进 tail。
    Atomics.store(this.tailView, 1, (tail + n) & 0xffffffff);
    return true;
  }
}

registerProcessor('jimmusic-audio-output', JimMusicAudioOutputProcessor);
