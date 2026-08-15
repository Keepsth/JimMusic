//! AGR-005：自动检测插件实时路径阻塞/分配违规的门禁。
//!
//! 加载已构建的 `null-output` 动态库，在计数分配器下连续执行写入路径：
//! - **分配违规**：实时写入路径的堆分配次数必须为 0；
//! - **阻塞违规**：单次写入与总墙钟时间必须在预算内。
//!
//! 依赖 `null-output` 动态库已被构建（`cargo build --workspace` 会构建它）；
//! 若找不到库则跳过（与 `host_ffi.rs` 相同的构建依赖约定）。

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use plugin_abi::output::{fns, OutputOpenParams, PcmFormat};
use plugin_abi::ErrorCode;

/// 全进程计数分配器：只在独立测试二进制中生效。
struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

// SAFETY: 直接转发给系统分配器。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: 转发。
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 转发。
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: 转发。
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 定位 `null-output` 动态库路径（与 host_ffi/output_ffi 同款推导逻辑）。
fn locate_null_output() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let debug = deps.parent()?;
    for name in [
        "libnull_output.so",
        "libnull_output.dylib",
        "null_output.dll",
    ] {
        let top = debug.join(name);
        if top.exists() {
            return Some(top);
        }
        let dep = deps.join(name);
        if dep.exists() {
            return Some(dep);
        }
    }
    None
}

#[test]
fn plugin_realtime_path_has_no_allocations_and_meets_blocking_budget() {
    let Some(path) = locate_null_output() else {
        eprintln!("null-output library not built; skipping AGR-005 gate");
        return;
    };
    // SAFETY: 加载已构建的插件动态库。
    let library = unsafe { libloading::Library::new(&path) }.expect("load null-output");
    // SAFETY: 符号名与插件 ABI 契约一致。
    let open: libloading::Symbol<'_, fns::Open> = unsafe {
        library
            .get(plugin_abi::output::symbols::OUTPUT_OPEN)
            .expect("open symbol")
    };
    let close: libloading::Symbol<'_, fns::Close> = unsafe {
        library
            .get(plugin_abi::output::symbols::OUTPUT_CLOSE)
            .expect("close symbol")
    };
    let write: libloading::Symbol<'_, fns::Write> = unsafe {
        library
            .get(plugin_abi::output::symbols::OUTPUT_WRITE)
            .expect("write symbol")
    };
    let play: libloading::Symbol<'_, fns::Control> = unsafe {
        library
            .get(plugin_abi::output::symbols::OUTPUT_PLAY)
            .expect("play symbol")
    };
    let flush: libloading::Symbol<'_, fns::Control> = unsafe {
        library
            .get(plugin_abi::output::symbols::OUTPUT_FLUSH)
            .expect("flush symbol")
    };

    let params = OutputOpenParams {
        sample_rate: 48_000,
        channels: 2,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 4096,
    };
    // SAFETY: params 指向有效 OutputOpenParams。
    let opened = unsafe { open(&params) };
    assert_eq!(opened.code, ErrorCode::Ok.as_i32(), "open failed");
    assert!(!opened.handle.is_null());
    assert_eq!(unsafe { play(opened.handle) }, ErrorCode::Ok.as_i32());
    let samples = vec![0_i16; 512 * 2];

    // 预热一轮（吸收惰性初始化），随后归零计数并测量。
    let _ = unsafe { write(opened.handle, samples.as_ptr(), 512) };
    assert_eq!(unsafe { flush(opened.handle) }, ErrorCode::Ok.as_i32());
    ALLOC_COUNT.store(0, Ordering::Relaxed);

    let started = Instant::now();
    let mut max_call = Duration::ZERO;
    let mut accepted_total = 0_i64;
    for _ in 0..2_000 {
        let call = Instant::now();
        // SAFETY: 句柄有效、pcm 指向 512*2 个 i16。
        accepted_total += unsafe { write(opened.handle, samples.as_ptr(), 512) } as i64;
        max_call = max_call.max(call.elapsed());
    }
    let elapsed = started.elapsed();
    let allocations = ALLOC_COUNT.load(Ordering::Relaxed);

    // SAFETY: 关闭并回收句柄。
    unsafe { close(opened.handle) };

    assert_eq!(
        allocations, 0,
        "realtime write path performed {allocations} heap allocations"
    );
    assert!(
        max_call < Duration::from_millis(10),
        "single write blocked for {max_call:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "2000 writes took {elapsed:?}"
    );
    assert!(accepted_total >= 0, "writes returned errors");
}
