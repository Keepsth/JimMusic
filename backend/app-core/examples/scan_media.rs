//! 演示：扫描一个目录，读取音频文件元数据。
//!
//! 用法：`cargo run -p app-core --example scan_media -- <dir>`

use app_core::MediaLibrary;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: scan_media <dir>");

    let lib = MediaLibrary::new();
    let tracks = lib.scan(&dir);

    println!("found {} track(s):", tracks.len());
    for t in tracks {
        let artist = t.artist.as_deref().unwrap_or("-");
        let album = t.album.as_deref().unwrap_or("-");
        let dur = t
            .duration
            .map(|d| format!("{:.2}s", d))
            .unwrap_or("-".into());
        println!(
            "  [{}] {} — {} ({} | {}Hz/{}ch)",
            dur,
            t.title,
            artist,
            album,
            t.sample_rate.unwrap_or(0),
            t.channels.unwrap_or(0),
        );
        println!("      {}", t.path);
    }
}
