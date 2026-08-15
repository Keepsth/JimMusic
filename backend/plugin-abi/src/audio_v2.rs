//! Audio ABI v2 的实时数据面 C 契约。
//!
//! 所有结构均为固定布局、借用 Host 缓冲且不拥有内存。字符串、动态配置、文件与网络
//! 操作只允许出现在非实时生命周期函数中，不能从 `process` 回调触发。

use std::ffi::c_void;

/// Audio ABI v2 独立版本号。它不与通用插件 ABI 的版本同步递增。
pub const AUDIO_ABI_V2: u32 = 2;
pub const MAX_AUDIO_PLANES: usize = 32;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMediaTypeV2 {
    Pcm = 0,
    Dsd = 1,
    Encoded = 2,
    Control = 3,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleTypeV2 {
    U8 = 0,
    I16 = 1,
    I24Packed = 2,
    I32 = 3,
    F32 = 4,
    F64 = 5,
    DsdU8 = 6,
    EncodedBytes = 7,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPackingV2 {
    Planar = 0,
    Interleaved = 1,
    Dop = 2,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEndianV2 {
    Little = 0,
    Big = 1,
    NotApplicable = 2,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTimelineStateV2 {
    Playing = 0,
    Paused = 1,
    Seeking = 2,
    Draining = 3,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRealtimeErrorV2 {
    Ok = 0,
    InvalidArgument = 1,
    UnsupportedFormat = 2,
    BufferTooSmall = 3,
    DeadlineMiss = 4,
    InvalidState = 5,
    NonFiniteOutput = 6,
    PluginFault = 7,
}

pub const AUDIO_FLAG_BIT_EXACT: u32 = 1 << 0;
pub const AUDIO_FLAG_SILENCE: u32 = 1 << 1;
pub const AUDIO_FLAG_DISCONTINUITY: u32 = 1 << 2;
pub const AUDIO_FLAG_END_OF_STREAM: u32 = 1 << 3;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormatV2 {
    pub struct_size: u32,
    pub media_type: AudioMediaTypeV2,
    pub sample_type: AudioSampleTypeV2,
    pub sample_rate: u32,
    pub channels: u32,
    /// 稳定的 Host 声道布局 ID。0 表示未指定，不可被当作 stereo。
    pub channel_layout: u64,
    pub packing: AudioPackingV2,
    pub endian: AudioEndianV2,
    pub flags: u32,
    pub reserved: [u32; 4],
}

impl AudioFormatV2 {
    pub const fn planar_f32(sample_rate: u32, channels: u32, channel_layout: u64) -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            media_type: AudioMediaTypeV2::Pcm,
            sample_type: AudioSampleTypeV2::F32,
            sample_rate,
            channels,
            channel_layout,
            packing: AudioPackingV2::Planar,
            endian: AudioEndianV2::NotApplicable,
            flags: 0,
            reserved: [0; 4],
        }
    }

    pub fn validate(&self) -> Result<(), AudioRealtimeErrorV2> {
        if self.struct_size < std::mem::size_of::<Self>() as u32
            || self.sample_rate == 0
            || self.channels == 0
            || self.channels as usize > MAX_AUDIO_PLANES
        {
            return Err(AudioRealtimeErrorV2::InvalidArgument);
        }
        if self.media_type == AudioMediaTypeV2::Dsd
            && !matches!(
                self.sample_type,
                AudioSampleTypeV2::DsdU8 | AudioSampleTypeV2::EncodedBytes
            )
        {
            return Err(AudioRealtimeErrorV2::UnsupportedFormat);
        }
        if self.media_type == AudioMediaTypeV2::Encoded
            && self.sample_type != AudioSampleTypeV2::EncodedBytes
        {
            return Err(AudioRealtimeErrorV2::UnsupportedFormat);
        }
        Ok(())
    }
}

/// 一个 Host 所有的音频平面。插件只能在 `capacity_bytes` 内读写。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioPlaneV2 {
    pub data: *mut u8,
    pub capacity_bytes: usize,
    pub stride_bytes: usize,
}

/// 仅在一次 process 调用中有效的 Host 缓冲视图。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBufferViewV2 {
    pub struct_size: u32,
    pub format: AudioFormatV2,
    pub planes: *mut AudioPlaneV2,
    pub plane_count: u32,
    pub capacity_frames: u32,
    pub valid_frames: u32,
    pub timestamp_frames: u64,
    pub flags: u32,
    pub reserved: [u32; 4],
}

impl AudioBufferViewV2 {
    /// 只检查布局和边界；不解引用插件可见指针。
    pub fn validate_layout(&self) -> Result<(), AudioRealtimeErrorV2> {
        self.format.validate()?;
        if self.struct_size < std::mem::size_of::<Self>() as u32
            || self.valid_frames > self.capacity_frames
            || self.plane_count == 0
            || self.plane_count as usize > MAX_AUDIO_PLANES
            || self.planes.is_null()
        {
            return Err(AudioRealtimeErrorV2::InvalidArgument);
        }
        let expected_planes = match self.format.packing {
            AudioPackingV2::Planar => self.format.channels,
            AudioPackingV2::Interleaved | AudioPackingV2::Dop => 1,
        };
        if self.plane_count != expected_planes {
            return Err(AudioRealtimeErrorV2::UnsupportedFormat);
        }
        Ok(())
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterValueTypeV2 {
    Boolean = 0,
    Integer = 1,
    Float = 2,
    Enum = 3,
}

/// 固定大小参数事件。字符串值和动态分配明确不属于实时 ABI。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ParameterEventV2 {
    pub parameter_id: u64,
    pub frame_offset: u32,
    pub value_type: ParameterValueTypeV2,
    pub value_bits: u64,
}

impl ParameterEventV2 {
    pub const fn float(parameter_id: u64, frame_offset: u32, value: f64) -> Self {
        Self {
            parameter_id,
            frame_offset,
            value_type: ParameterValueTypeV2::Float,
            value_bits: value.to_bits(),
        }
    }

    pub const fn float_value(&self) -> Option<f64> {
        match self.value_type {
            ParameterValueTypeV2::Float => Some(f64::from_bits(self.value_bits)),
            _ => None,
        }
    }
}

/// Host 提供的实时安全计数器，插件只做原子加法（具体原子 ABI 由 Host 包装）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RealtimeDiagnosticsV2 {
    pub user_data: *mut c_void,
    pub increment: Option<unsafe extern "C" fn(*mut c_void, u32, u64)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProcessContextV2 {
    pub struct_size: u32,
    pub timeline_frame: u64,
    pub block_frames: u32,
    pub state: AudioTimelineStateV2,
    pub flags: u32,
    pub parameter_events: *const ParameterEventV2,
    pub parameter_event_count: u32,
    pub deadline_ns: u64,
    pub diagnostics: RealtimeDiagnosticsV2,
    pub reserved: [u64; 4],
}

pub type AudioNodeHandleV2 = *mut c_void;
pub type AudioNodeProcessV2 = unsafe extern "C" fn(
    AudioNodeHandleV2,
    *const ProcessContextV2,
    *const AudioBufferViewV2,
    u32,
    *mut AudioBufferViewV2,
    u32,
) -> AudioRealtimeErrorV2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_f32_is_valid() {
        let format = AudioFormatV2::planar_f32(48_000, 2, 2);
        assert_eq!(format.validate(), Ok(()));
        assert_eq!(format.packing, AudioPackingV2::Planar);
    }

    #[test]
    fn dsd_cannot_claim_pcm_sample_type() {
        let mut format = AudioFormatV2::planar_f32(2_822_400, 2, 2);
        format.media_type = AudioMediaTypeV2::Dsd;
        assert_eq!(
            format.validate(),
            Err(AudioRealtimeErrorV2::UnsupportedFormat)
        );
    }

    #[test]
    fn view_rejects_wrong_plane_count() {
        let mut plane = AudioPlaneV2 {
            data: std::ptr::null_mut(),
            capacity_bytes: 0,
            stride_bytes: 4,
        };
        let view = AudioBufferViewV2 {
            struct_size: std::mem::size_of::<AudioBufferViewV2>() as u32,
            format: AudioFormatV2::planar_f32(48_000, 2, 2),
            planes: &mut plane,
            plane_count: 1,
            capacity_frames: 128,
            valid_frames: 128,
            timestamp_frames: 0,
            flags: 0,
            reserved: [0; 4],
        };
        assert_eq!(
            view.validate_layout(),
            Err(AudioRealtimeErrorV2::UnsupportedFormat)
        );
    }

    #[test]
    fn parameter_float_round_trips_without_allocation() {
        let event = ParameterEventV2::float(7, 63, 0.25);
        assert_eq!(event.float_value(), Some(0.25));
    }
}
