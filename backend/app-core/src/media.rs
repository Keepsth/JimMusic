//! 媒体库：本地音频扫描、元数据读取与分类。
//!
//! [`MediaLibrary`] 递归扫描目录，识别常见音频文件（`.mp3/.aac/.flac/.wav/.ogg/.m4a/.pcm`），
//! 借助 Symphonia 解码器插件读取元数据（标题/艺术家/专辑/时长/采样率/声道），
//! 返回可序列化的 [`Track`] 列表，供 Flutter 前端或插件管理器消费。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 受支持的音频扩展名。
const AUDIO_EXTS: &[&str] = &["mp3", "aac", "flac", "wav", "ogg", "m4a", "pcm"];

/// 一条媒体库音轨记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    /// 文件绝对路径。
    pub path: String,
    /// 标题（缺省时回退为文件名）。
    pub title: String,
    /// 艺术家。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// 专辑。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// 时长（秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// 采样率（Hz）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// 声道数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
}

/// 本地媒体库。
#[derive(Default)]
pub struct MediaLibrary;

impl MediaLibrary {
    /// 创建媒体库。
    pub fn new() -> Self {
        Self
    }

    /// 递归扫描目录，返回识别出的音轨列表。
    ///
    /// `dir` 不存在时返回空列表（不报错，体现断网/无目录时的可用性）。
    pub fn scan(&self, dir: impl AsRef<Path>) -> Vec<Track> {
        self.scan_bounded(dir, usize::MAX).0
    }

    /// Scan with an explicit track count ceiling so an untrusted or very large
    /// directory tree cannot grow the in-memory result without bound.
    pub fn scan_bounded(&self, dir: impl AsRef<Path>, limit: usize) -> (Vec<Track>, bool) {
        let dir = dir.as_ref();
        let mut tracks = Vec::new();
        let mut truncated = false;

        if !dir.is_dir() {
            return (tracks, false);
        }

        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !is_audio_file(path) {
                continue;
            }
            if let Some(track) = Self::read_track(path) {
                if tracks.len() == limit {
                    truncated = true;
                    break;
                }
                tracks.push(track);
            }
        }

        // 按标题排序，稳定输出。
        tracks.sort_by(|a, b| a.title.cmp(&b.title));
        (tracks, truncated)
    }

    /// 读取单个音频文件的元数据并构造 [`Track`]。
    pub fn read_track(path: &Path) -> Option<Track> {
        let meta = symphonia_decoder::read_metadata(path).ok()?;
        let title = meta.title.clone().unwrap_or_else(|| default_title(path));
        Some(Track {
            path: path.to_string_lossy().into_owned(),
            title,
            artist: meta.artist,
            album: meta.album,
            duration: meta.duration,
            sample_rate: meta.sample_rate,
            channels: meta.channels,
        })
    }
}

/// 判断文件是否为受支持的音频文件。
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// 从文件路径推导缺省标题（去掉扩展名的文件名）。
fn default_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 生成一个最小的 WAV 文件（16-bit 静音 PCM，用于扫描测试）。
    fn write_wav(path: &Path) {
        let sr = 8000u32;
        let n = 800usize;
        let data = vec![0u8; n * 2]; // 静音

        let dl = data.len() as u32;
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + dl).to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&sr.to_le_bytes()).unwrap();
        f.write_all(&(sr * 2).to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&dl.to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
    }

    #[test]
    fn scan_discovers_audio_files() {
        let dir = std::env::temp_dir().join("jimmusic_scan_test");
        std::fs::create_dir_all(&dir).unwrap();
        write_wav(&dir.join("song_a.wav"));
        write_wav(&dir.join("song_b.wav"));
        // 非音频文件应被忽略。
        std::fs::write(dir.join("readme.txt"), b"hello").unwrap();

        let lib = MediaLibrary::new();
        let tracks = lib.scan(&dir);
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|t| t.sample_rate == Some(8000)));
        assert!(tracks.iter().all(|t| t.channels == Some(1)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_missing_dir_returns_empty() {
        let lib = MediaLibrary::new();
        assert!(lib.scan("/nonexistent/dir/xyz").is_empty());
    }

    #[test]
    fn audio_ext_detection() {
        assert!(is_audio_file(Path::new("a.mp3")));
        assert!(is_audio_file(Path::new("a.FLAC")));
        assert!(!is_audio_file(Path::new("a.txt")));
        assert!(!is_audio_file(Path::new("a")));
    }
}
