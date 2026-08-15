//! 演示：动态加载一个插件库并调用其能力。
//!
//! 用法：`cargo run -p app-core --example load_plugin -- <path-to-plugin.so>`
//!
//! 该示例完整走一遍核心的插件热插拔链路：加载 → ABI 校验 → 初始化 → 调用 → 卸载。

use app_core::PluginManager;

fn main() {
    app_core::init_logging();

    let path = std::env::args()
        .nth(1)
        .expect("usage: load_plugin <path-to-plugin.so>");

    let mut manager = PluginManager::new();

    // 1. 加载。
    let plugin = match manager.load(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("load failed: {e}");
            std::process::exit(1);
        }
    };

    let meta = plugin.meta().clone();
    println!("loaded plugin:");
    println!("  name    = {}", meta.name);
    println!("  version = {}", meta.version);
    println!("  author  = {}", meta.author);
    println!("  kind    = {:?}", meta.kind);
    println!("  path    = {}", meta.path.display());

    // 2. 调用：根据插件种类选择探测操作。
    let op = match meta.kind {
        app_core::PluginKind::UiBridge => "ping",
        app_core::PluginKind::Decoder => "formats",
        _ => "ping",
    };
    match plugin.invoke(op, &[]) {
        Ok(out) => println!("invoke `{op}` -> {}", String::from_utf8_lossy(&out)),
        Err(e) => println!("invoke `{op}` failed: {e}"),
    }

    // 3. 卸载。
    if let Err(e) = manager.unload(&meta.name) {
        eprintln!("unload failed: {e}");
        std::process::exit(1);
    }
    println!("unloaded `{}`", meta.name);
    assert_eq!(manager.len(), 0);
    println!("OK");
}
