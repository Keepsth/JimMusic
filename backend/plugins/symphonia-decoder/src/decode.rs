//! 真实音频解码与元数据读取（基于 symphonia 0.6）。
//!
//! 该模块是 Symphonia 解码器插件的核心实现：纯 Rust、静态链接，支持
//! MP3 / AAC / FLAC / WAV / OGG(Vorbis) / PCM 等格式的解码与元数据（ID3 / APE / Vorbis 注释）
//! 读取。既作为插件 C ABI 的底层能力，也以普通 Rust API 对外暴露。

use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, StandardTag};

/// 解码相关错误。
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// 文件打开/读取失败。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 探测/解码失败（symphonia 错误）。
    #[error("decode error: {0}")]
    Symphonia(#[from] SymphoniaError),
    /// 不支持或缺少默认音轨。
    #[error("no default audio track found")]
    NoTrack,
    /// 无法确定音频参数。
    #[error("missing audio codec parameters")]
    NoAudioParams,
}

/// 音轨元数据。
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    /// 标题。
    pub title: Option<String>,
    /// 艺术家。
    pub artist: Option<String>,
    /// 专辑。
    pub album: Option<String>,
    /// 时长（秒）。
    pub duration: Option<f64>,
    /// 采样率（Hz）。
    pub sample_rate: Option<u32>,
    /// 声道数。
    pub channels: Option<u16>,
}

/// 解码出的 PCM 样本（16-bit，交错声道）。
pub struct DecodedAudio {
    /// 采样率（Hz）。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 采样帧总数。
    pub frames: u64,
    /// 交错 PCM 样本（i16）。
    pub samples: Vec<i16>,
}

/// 增量解码得到的一个有界 PCM 块。
#[derive(Debug)]
pub struct DecodedChunk {
    pub sample_rate: u32,
    pub channels: u16,
    pub start_frame: u64,
    pub samples: Vec<i16>,
}

impl DecodedChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// 持有 demuxer/decoder 状态的增量解码器。它一次最多向调用方返回 `max_frames`，
/// 不会把整首 PCM 常驻内存。
pub struct StreamingDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    duration_frames: Option<u64>,
    next_frame: u64,
    pending: Vec<i16>,
}

impl StreamingDecoder {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let file = std::fs::File::open(path)?;
        Self::from_source(file)
    }

    pub fn from_source<R: MediaSource + 'static>(reader: R) -> Result<Self, DecodeError> {
        let mss = MediaSourceStream::new(Box::new(reader), Default::default());
        let hint = Hint::new();
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecodeError::NoTrack)?
            .clone();
        let codec_params = track
            .codec_params
            .as_ref()
            .ok_or(DecodeError::NoAudioParams)?;
        let audio_params = codec_params.audio().ok_or(DecodeError::NoAudioParams)?;
        let sample_rate = audio_params.sample_rate.unwrap_or(44_100);
        let channels = audio_params
            .channels
            .as_ref()
            .map(|channels| channels.count())
            .unwrap_or(2) as u16;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &AudioDecoderOptions::default())?;
        Ok(Self {
            format,
            decoder,
            track_id: track.id,
            sample_rate,
            channels,
            duration_frames: track.duration.map(|duration| duration.get()),
            next_frame: 0,
            pending: Vec::new(),
        })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }

    pub const fn duration_frames(&self) -> Option<u64> {
        self.duration_frames
    }

    /// 丢弃到目标帧。Seek 通过重新打开源后有界跳读实现，格式插件以后可覆写为原生 seek。
    pub fn skip_to_frame(&mut self, target: u64, max_frames: usize) -> Result<(), DecodeError> {
        while self.next_frame < target {
            let Some(mut chunk) = self.next_chunk(max_frames)? else {
                break;
            };
            let chunk_end = chunk.start_frame + chunk.frames() as u64;
            if chunk_end > target {
                let skip = (target - chunk.start_frame) as usize * self.channels as usize;
                // `next_chunk` may already have left decoded samples in
                // `self.pending`. Preserve them after the unconsumed suffix of
                // this chunk; replacing the buffer here silently dropped audio
                // after seeks that landed inside a decoded packet.
                let mut suffix = chunk.samples.split_off(skip);
                suffix.append(&mut self.pending);
                self.pending = suffix;
                self.next_frame = target;
                break;
            }
        }
        Ok(())
    }

    pub fn next_chunk(&mut self, max_frames: usize) -> Result<Option<DecodedChunk>, DecodeError> {
        let max_frames = max_frames.max(1);
        let max_samples = max_frames.saturating_mul(self.channels as usize);
        while self.pending.is_empty() {
            let Some(packet) = self.format.next_packet()? else {
                return Ok(None);
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = self.decoder.decode(&packet)?;
            push_audio_buffer(decoded, &mut self.pending);
        }

        let take = self.pending.len().min(max_samples);
        let remainder = self.pending.split_off(take);
        let samples = std::mem::replace(&mut self.pending, remainder);
        let start_frame = self.next_frame;
        self.next_frame += (samples.len() / self.channels as usize) as u64;
        Ok(Some(DecodedChunk {
            sample_rate: self.sample_rate,
            channels: self.channels,
            start_frame,
            samples,
        }))
    }
}

/// 读取音轨元数据。
pub fn read_metadata(path: &Path) -> Result<TrackMetadata, DecodeError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let hint = Hint::new();
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;

    let mut meta = TrackMetadata::default();

    // 读取元数据 tag。
    if let Some(rev) = format.metadata().current() {
        for tag in &rev.media.tags {
            if let Some(std) = &tag.std {
                match std {
                    StandardTag::TrackTitle(v) => meta.title = Some(v.to_string()),
                    StandardTag::Artist(v) => meta.artist = Some(v.to_string()),
                    StandardTag::Album(v) => meta.album = Some(v.to_string()),
                    _ => {}
                }
            }
        }
    }

    // 从默认音轨推断采样率/声道/时长。
    if let Some(track) = format.default_track(TrackType::Audio) {
        if let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) {
            meta.sample_rate = params.sample_rate;
            meta.channels = params.channels.as_ref().map(|c| c.count() as u16);
        }
        meta.duration = track_duration_secs(track);
    }

    Ok(meta)
}

/// 将音频文件完整解码为 16-bit PCM 样本。
pub fn decode_file(path: &Path) -> Result<DecodedAudio, DecodeError> {
    let file = std::fs::File::open(path)?;
    decode_from_reader(file)
}

/// 从任意可读可寻址的数据源（如 `std::io::Cursor<Vec<u8>>`、文件、网络缓冲）
/// 解码为 16-bit PCM 样本。
///
/// 配合 [`crate::decode`] 与 IPFS 流式下载，可对「已下载到内存的字节」直接解码，
/// 实现边下载边播放（无需先落盘）。
pub fn decode_from_reader<R: MediaSource + 'static>(
    reader: R,
) -> Result<DecodedAudio, DecodeError> {
    let mss = MediaSourceStream::new(Box::new(reader), Default::default());
    let hint = Hint::new();
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoTrack)?
        .clone();

    let codec_params = track
        .codec_params
        .as_ref()
        .ok_or(DecodeError::NoAudioParams)?;
    let audio_params = codec_params.audio().ok_or(DecodeError::NoAudioParams)?;

    let sample_rate = audio_params.sample_rate.unwrap_or(44_100);
    let channels = audio_params
        .channels
        .as_ref()
        .map(|c| c.count())
        .unwrap_or(2) as u16;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())?;

    let mut samples: Vec<i16> = Vec::new();

    while let Some(packet) = format.next_packet()? {
        let decoded = decoder.decode(&packet)?;
        push_audio_buffer(decoded, &mut samples);
    }

    let frames = (samples.len() / channels.max(1) as usize) as u64;
    Ok(DecodedAudio {
        sample_rate,
        channels,
        frames,
        samples,
    })
}

/// 从 [`symphonia::core::formats::Track`] 计算时长（秒）。
fn track_duration_secs(track: &symphonia::core::formats::Track) -> Option<f64> {
    let dur = track.duration?;
    if let Some(tb) = track.time_base {
        if let Some(time) = tb.calc_duration(dur) {
            return Some(time.as_secs_f64());
        }
    }
    None
}

/// 将 symphonia 解码缓冲追加为 i16 交错样本。
///
/// 注意：`copy_to_vec_interleaved` 会 *resize* 目标向量（覆盖而非追加），
/// 因此这里先写入临时缓冲再 `extend`，以避免跨数据包丢失样本。
fn push_audio_buffer(buf: symphonia::core::audio::GenericAudioBufferRef<'_>, out: &mut Vec<i16>) {
    let mut tmp: Vec<i16> = Vec::new();
    buf.copy_to_vec_interleaved::<i16>(&mut tmp);
    out.extend_from_slice(&tmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 生成一个 1 秒、44.1kHz、单声道、220Hz 正弦波的 WAV 文件用于真实解码测试。
    fn write_test_wav(path: &Path) {
        let sample_rate = 44_100u32;
        let channels = 1u16;
        let seconds = 1.0f64;
        let n_samples = (sample_rate as f64 * seconds) as usize;

        let mut data = Vec::with_capacity(n_samples * 2);
        for i in 0..n_samples {
            let t = i as f64 / sample_rate as f64;
            let v = (2.0 * std::f64::consts::PI * 220.0 * t).sin();
            let sample = (v * i16::MAX as f64 * 0.5) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
        }

        let data_len = data.len() as u32;
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;

        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
        f.write_all(&channels.to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&byte_rate.to_le_bytes()).unwrap();
        f.write_all(&block_align.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
    }

    #[test]
    fn decode_wav_produces_pcm() {
        let dir = std::env::temp_dir().join("jimmusic_wav_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_test_wav(&path);

        let meta = read_metadata(&path).unwrap();
        assert_eq!(meta.sample_rate, Some(44_100));
        assert_eq!(meta.channels, Some(1));

        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.frames, 44_100);
        assert_eq!(audio.samples.len(), 44_100);
        // 正弦波样本应非零。
        assert!(audio.samples.iter().any(|&s| s != 0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_from_memory_reader() {
        // 将 WAV 字节写入内存，再从 Cursor 直接解码（模拟「下载到内存后边下载边播放」）。
        let dir = std::env::temp_dir().join("jimmusic_wav_mem_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_test_wav(&path);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let reader = std::io::Cursor::new(bytes);
        let audio = decode_from_reader(reader).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.frames, 44_100);
        assert!(audio.samples.iter().any(|&s| s != 0));
    }

    #[test]
    fn streaming_decoder_keeps_chunks_bounded() {
        let dir = std::env::temp_dir().join("jimmusic_wav_stream_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_test_wav(&path);

        let mut decoder = StreamingDecoder::open(&path).unwrap();
        let mut frames = 0usize;
        while let Some(chunk) = decoder.next_chunk(256).unwrap() {
            assert!(chunk.frames() <= 256);
            frames += chunk.frames();
        }
        assert_eq!(frames, 44_100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streaming_decoder_can_skip_without_retaining_prefix() {
        let dir = std::env::temp_dir().join("jimmusic_wav_seek_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_test_wav(&path);

        let mut decoder = StreamingDecoder::open(&path).unwrap();
        decoder.skip_to_frame(22_050, 256).unwrap();
        let mut next_start = 22_050u64;
        let mut remaining_frames = 0usize;
        while let Some(chunk) = decoder.next_chunk(256).unwrap() {
            assert_eq!(chunk.start_frame, next_start);
            assert!(chunk.frames() <= 256);
            next_start += chunk.frames() as u64;
            remaining_frames += chunk.frames();
        }
        assert_eq!(
            remaining_frames, 22_050,
            "seek must not drop buffered audio"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
