//! 本地优先媒体库：统一 Track/Source、可用性、歌单、队列和恢复会话。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jimmusic_protocol::{MusicManifestV1, MusicRenditionV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::media::Track;
use crate::storage::{AtomicJsonStore, StorageError};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrackSourceKind {
    LocalFile,
    CachedObject,
    Ipfs,
    Community,
    MemoryImport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Available,
    Missing,
    Offline,
    RequiresDecoder,
    IntegrityFailed,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackSourceV1 {
    pub source_id: String,
    pub kind: TrackSourceKind,
    pub uri: String,
    pub content_cid: Option<String>,
    pub rendition_id: Option<String>,
    pub container: String,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u16>,
    pub channels: Option<u16>,
    pub byte_length: Option<u64>,
    pub lossless: bool,
    pub original: bool,
    pub streamable: bool,
    pub availability: SourceAvailability,
    pub last_checked_at: i64,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Imported,
    Indexed,
    Missing,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryTrackV1 {
    pub track_id: String,
    pub work_id: Option<String>,
    pub release_id: Option<String>,
    pub manifest_cid: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub tags: Vec<String>,
    pub sources: Vec<TrackSourceV1>,
    pub selected_source_id: Option<String>,
    pub scan_state: ScanState,
    pub favorite: bool,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistV1 {
    pub playlist_id: String,
    pub name: String,
    pub track_ids: Vec<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaybackSessionV1 {
    pub current_track_id: Option<String>,
    pub queue: Vec<String>,
    pub position_seconds: f64,
    pub selected_audio_path: Option<String>,
    pub volume: f64,
    pub muted: bool,
    /// 恢复快照永远为 false，避免异常退出后自动出声。
    pub auto_play: bool,
}

impl Default for PlaybackSessionV1 {
    fn default() -> Self {
        Self {
            current_track_id: None,
            queue: Vec::new(),
            position_seconds: 0.0,
            selected_audio_path: None,
            volume: 1.0,
            muted: false,
            auto_play: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryRepositoryState {
    schema_version: u16,
    music_directory: Option<String>,
    tracks: BTreeMap<String, LibraryTrackV1>,
    playlists: BTreeMap<String, PlaylistV1>,
    session: PlaybackSessionV1,
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("track `{0}` does not exist")]
    TrackNotFound(String),
    #[error("playlist `{0}` does not exist")]
    PlaylistNotFound(String),
    #[error("playlist name must not be empty")]
    EmptyPlaylistName,
    #[error("no compatible rendition; required codecs: {0:?}")]
    NoCompatibleRendition(Vec<String>),
    #[error("music directory is not writable: {0}")]
    DirectoryNotWritable(String),
}

pub struct LibraryService {
    store: AtomicJsonStore<LibraryRepositoryState>,
}

impl LibraryService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        Ok(Self {
            store: AtomicJsonStore::open(
                path,
                LibraryRepositoryState {
                    schema_version: 1,
                    music_directory: None,
                    tracks: BTreeMap::new(),
                    playlists: BTreeMap::new(),
                    session: PlaybackSessionV1::default(),
                },
            )?,
        })
    }

    pub fn import_local(
        &self,
        track: Track,
        timestamp: i64,
    ) -> Result<LibraryTrackV1, LibraryError> {
        let track_id = stable_id("local-track", track.path.as_bytes());
        let source_id = stable_id("local-source", track.path.as_bytes());
        let path = Path::new(&track.path);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let available = path.exists();
        let source = TrackSourceV1 {
            source_id: source_id.clone(),
            kind: TrackSourceKind::LocalFile,
            uri: track.path.clone(),
            content_cid: None,
            rendition_id: None,
            container: extension.clone(),
            codec: extension,
            sample_rate: track.sample_rate,
            bit_depth: None,
            channels: track.channels,
            byte_length: path.metadata().ok().map(|metadata| metadata.len()),
            lossless: matches!(
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("flac" | "wav")
            ),
            original: true,
            streamable: true,
            availability: if available {
                SourceAvailability::Available
            } else {
                SourceAvailability::Missing
            },
            last_checked_at: timestamp,
            unavailable_reason: (!available).then(|| "local file is missing".into()),
        };
        let item = LibraryTrackV1 {
            track_id: track_id.clone(),
            work_id: None,
            release_id: None,
            manifest_cid: None,
            title: track.title,
            artists: track.artist.into_iter().collect(),
            album: track.album.unwrap_or_default(),
            duration_ms: track
                .duration
                .map(|seconds| (seconds.max(0.0) * 1000.0) as u64),
            tags: Vec::new(),
            sources: vec![source],
            selected_source_id: Some(source_id),
            scan_state: if available {
                ScanState::Indexed
            } else {
                ScanState::Missing
            },
            favorite: false,
            added_at: timestamp,
            updated_at: timestamp,
        };
        Ok(self.store.transact(|state| {
            let existing_favorite = state
                .tracks
                .get(&track_id)
                .is_some_and(|track| track.favorite);
            let mut item = item.clone();
            item.favorite = existing_favorite;
            state.tracks.insert(track_id, item.clone());
            Ok(item)
        })?)
    }

    pub fn import_manifest(
        &self,
        manifest_cid: String,
        manifest: &MusicManifestV1,
        timestamp: i64,
    ) -> Result<LibraryTrackV1, LibraryError> {
        let track_id = stable_id("manifest-track", manifest_cid.as_bytes());
        let sources: Vec<_> = manifest
            .renditions
            .iter()
            .map(|rendition| rendition_source(rendition, timestamp))
            .collect();
        let selected_source_id = sources
            .iter()
            .find(|source| source.original)
            .or_else(|| sources.first())
            .map(|source| source.source_id.clone());
        let item = LibraryTrackV1 {
            track_id: track_id.clone(),
            work_id: Some(manifest.work_id.clone()),
            release_id: Some(manifest.release_id.clone()),
            manifest_cid: Some(manifest_cid),
            title: manifest.title.clone(),
            artists: manifest.artists.clone(),
            album: manifest.album.clone(),
            duration_ms: Some(manifest.duration_ms),
            tags: manifest.tags.clone(),
            sources,
            selected_source_id,
            scan_state: ScanState::Indexed,
            favorite: false,
            added_at: timestamp,
            updated_at: timestamp,
        };
        Ok(self.store.transact(|state| {
            if let Some(existing) = state.tracks.get(&track_id) {
                let mut replacement = item.clone();
                replacement.favorite = existing.favorite;
                replacement.added_at = existing.added_at;
                state.tracks.insert(track_id, replacement.clone());
                Ok(replacement)
            } else {
                state.tracks.insert(track_id, item.clone());
                Ok(item.clone())
            }
        })?)
    }

    pub fn tracks(&self) -> Vec<LibraryTrackV1> {
        self.store.snapshot().tracks.into_values().collect()
    }

    pub fn track(&self, track_id: &str) -> Option<LibraryTrackV1> {
        self.store.snapshot().tracks.get(track_id).cloned()
    }

    pub fn search(&self, query: &str) -> Vec<LibraryTrackV1> {
        let query = query.trim().to_lowercase();
        self.tracks()
            .into_iter()
            .filter(|track| {
                query.is_empty()
                    || track.title.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
                    || track
                        .artists
                        .iter()
                        .any(|artist| artist.to_lowercase().contains(&query))
                    || track
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn refresh_availability(&self, timestamp: i64) -> Result<usize, LibraryError> {
        Ok(self.store.transact(|state| {
            let mut missing = 0;
            for track in state.tracks.values_mut() {
                for source in &mut track.sources {
                    if source.kind == TrackSourceKind::LocalFile {
                        let exists = Path::new(&source.uri).exists();
                        source.availability = if exists {
                            SourceAvailability::Available
                        } else {
                            missing += 1;
                            SourceAvailability::Missing
                        };
                        source.unavailable_reason =
                            (!exists).then(|| "local file is missing".into());
                        source.last_checked_at = timestamp;
                    }
                }
                track.scan_state = if track
                    .sources
                    .iter()
                    .any(|source| source.availability == SourceAvailability::Available)
                {
                    ScanState::Indexed
                } else {
                    ScanState::Missing
                };
                track.updated_at = timestamp;
            }
            Ok(missing)
        })?)
    }

    pub fn choose_source(
        &self,
        track_id: &str,
        supported_codecs: &BTreeSet<String>,
        allow_network: bool,
    ) -> Result<TrackSourceV1, LibraryError> {
        let track = self
            .track(track_id)
            .ok_or_else(|| LibraryError::TrackNotFound(track_id.into()))?;
        let mut candidates: Vec<_> = track
            .sources
            .iter()
            .filter(|source| {
                supported_codecs.contains(&source.codec.to_ascii_lowercase())
                    && source.availability != SourceAvailability::IntegrityFailed
                    && (allow_network
                        || matches!(
                            source.kind,
                            TrackSourceKind::LocalFile | TrackSourceKind::CachedObject
                        ))
            })
            .cloned()
            .collect();
        candidates.sort_by_key(|source| {
            (
                !matches!(
                    source.kind,
                    TrackSourceKind::LocalFile | TrackSourceKind::CachedObject
                ),
                !source.original,
                !source.lossless,
                source.byte_length.unwrap_or(u64::MAX),
            )
        });
        let selected = candidates.into_iter().next().ok_or_else(|| {
            LibraryError::NoCompatibleRendition(
                track
                    .sources
                    .iter()
                    .map(|source| source.codec.clone())
                    .collect(),
            )
        })?;
        self.store.transact(|state| {
            state
                .tracks
                .get_mut(track_id)
                .expect("track checked above")
                .selected_source_id = Some(selected.source_id.clone());
            Ok(())
        })?;
        Ok(selected)
    }

    pub fn set_favorite(&self, track_id: &str, favorite: bool) -> Result<(), LibraryError> {
        if !self.store.snapshot().tracks.contains_key(track_id) {
            return Err(LibraryError::TrackNotFound(track_id.into()));
        }
        self.store.transact(|state| {
            state
                .tracks
                .get_mut(track_id)
                .expect("checked above")
                .favorite = favorite;
            Ok(())
        })?;
        Ok(())
    }

    pub fn create_playlist(&self, name: &str, timestamp: i64) -> Result<PlaylistV1, LibraryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(LibraryError::EmptyPlaylistName);
        }
        Ok(self.store.transact(|state| {
            let mut discriminator = state.playlists.len();
            let id = loop {
                let candidate = stable_id(
                    "playlist",
                    format!("{name}:{timestamp}:{discriminator}").as_bytes(),
                );
                if !state.playlists.contains_key(&candidate) {
                    break candidate;
                }
                discriminator += 1;
            };
            let playlist = PlaylistV1 {
                playlist_id: id.clone(),
                name: name.into(),
                track_ids: Vec::new(),
                updated_at: timestamp,
            };
            state.playlists.insert(id, playlist.clone());
            Ok(playlist)
        })?)
    }

    pub fn update_playlist(
        &self,
        playlist_id: &str,
        name: Option<String>,
        track_ids: Option<Vec<String>>,
        timestamp: i64,
    ) -> Result<PlaylistV1, LibraryError> {
        let snapshot = self.store.snapshot();
        if !snapshot.playlists.contains_key(playlist_id) {
            return Err(LibraryError::PlaylistNotFound(playlist_id.into()));
        }
        if name.as_ref().is_some_and(|name| name.trim().is_empty()) {
            return Err(LibraryError::EmptyPlaylistName);
        }
        if let Some(ids) = &track_ids {
            if let Some(missing) = ids.iter().find(|id| !snapshot.tracks.contains_key(*id)) {
                return Err(LibraryError::TrackNotFound(missing.clone()));
            }
        }
        Ok(self.store.transact(|state| {
            let playlist = state.playlists.get_mut(playlist_id).expect("checked above");
            if let Some(name) = name {
                playlist.name = name.trim().into();
            }
            if let Some(track_ids) = track_ids {
                playlist.track_ids = track_ids;
            }
            playlist.updated_at = timestamp;
            Ok(playlist.clone())
        })?)
    }

    pub fn playlists(&self) -> Vec<PlaylistV1> {
        self.store.snapshot().playlists.into_values().collect()
    }

    pub fn remove_playlist(&self, playlist_id: &str) -> Result<(), LibraryError> {
        if !self.store.snapshot().playlists.contains_key(playlist_id) {
            return Err(LibraryError::PlaylistNotFound(playlist_id.into()));
        }
        self.store.transact(|state| {
            state.playlists.remove(playlist_id);
            Ok(())
        })?;
        Ok(())
    }

    pub fn save_session(&self, mut session: PlaybackSessionV1) -> Result<(), LibraryError> {
        session.volume = session.volume.clamp(0.0, 1.0);
        session.position_seconds = session.position_seconds.max(0.0);
        session.auto_play = false;
        let tracks = self.store.snapshot().tracks;
        session
            .queue
            .retain(|track_id| tracks.contains_key(track_id));
        if session
            .current_track_id
            .as_ref()
            .is_some_and(|track_id| !tracks.contains_key(track_id))
        {
            session.current_track_id = None;
            session.position_seconds = 0.0;
        }
        self.store.transact(|state| {
            state.session = session;
            Ok(())
        })?;
        Ok(())
    }

    pub fn session(&self) -> PlaybackSessionV1 {
        let mut session = self.store.snapshot().session;
        session.auto_play = false;
        session
    }

    pub fn set_music_directory(&self, directory: &Path) -> Result<(), LibraryError> {
        std::fs::create_dir_all(directory)
            .map_err(|_| LibraryError::DirectoryNotWritable(directory.display().to_string()))?;
        let probe = directory.join(".jimmusic-write-probe");
        if std::fs::write(&probe, b"probe").is_err() {
            return Err(LibraryError::DirectoryNotWritable(
                directory.display().to_string(),
            ));
        }
        let _ = std::fs::remove_file(probe);
        self.store.transact(|state| {
            state.music_directory = Some(directory.to_string_lossy().into_owned());
            Ok(())
        })?;
        Ok(())
    }

    pub fn music_directory(&self) -> Option<String> {
        self.store.snapshot().music_directory
    }
}

fn rendition_source(rendition: &MusicRenditionV1, timestamp: i64) -> TrackSourceV1 {
    TrackSourceV1 {
        source_id: stable_id("rendition", rendition.rendition_id.as_bytes()),
        kind: TrackSourceKind::Ipfs,
        uri: format!("ipfs://{}", rendition.content_cid),
        content_cid: Some(rendition.content_cid.clone()),
        rendition_id: Some(rendition.rendition_id.clone()),
        container: rendition.container.to_ascii_lowercase(),
        codec: rendition.codec.to_ascii_lowercase(),
        sample_rate: Some(rendition.sample_rate),
        bit_depth: Some(rendition.bit_depth),
        channels: Some(rendition.channels),
        byte_length: Some(rendition.byte_length),
        lossless: rendition.lossless,
        original: rendition.original,
        streamable: rendition.streamable,
        availability: SourceAvailability::Offline,
        last_checked_at: timestamp,
        unavailable_reason: Some("provider resolution has not completed".into()),
    }
}

fn stable_id(domain: &str, value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(value);
    format!("jm_{}", &hex::encode(hasher.finalize())[..24])
}

#[cfg(test)]
mod tests {
    use super::*;
    use jimmusic_protocol::{LicenseDeclaration, SCHEMA_V1};

    fn track(path: &Path) -> Track {
        Track {
            path: path.to_string_lossy().into_owned(),
            title: "Title".into(),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration: Some(1.0),
            sample_rate: Some(44_100),
            channels: Some(2),
        }
    }

    fn manifest() -> MusicManifestV1 {
        MusicManifestV1 {
            schema_version: SCHEMA_V1,
            work_id: "work".into(),
            release_id: "release".into(),
            title: "Remote".into(),
            artists: vec!["Artist".into()],
            album: "Album".into(),
            track_number: None,
            disc_number: None,
            duration_ms: 1_000,
            language: "en".into(),
            genres: Vec::new(),
            tags: vec!["ambient".into()],
            cover_cid: None,
            lyrics_cid: None,
            credits: BTreeMap::new(),
            license: LicenseDeclaration {
                identifier: "CC0-1.0".into(),
                rights_statement: None,
                allows_redistribution: true,
            },
            content_labels: Vec::new(),
            renditions: vec![
                MusicRenditionV1 {
                    rendition_id: "flac".into(),
                    content_cid: "bafyflac".into(),
                    container: "flac".into(),
                    codec: "flac".into(),
                    profile: String::new(),
                    sample_rate: 96_000,
                    bit_depth: 24,
                    channels: 2,
                    channel_layout: "stereo".into(),
                    duration_ms: 1_000,
                    byte_length: 1_000,
                    lossless: true,
                    original: true,
                    streamable: true,
                },
                MusicRenditionV1 {
                    rendition_id: "opus".into(),
                    content_cid: "bafyopus".into(),
                    container: "ogg".into(),
                    codec: "opus".into(),
                    profile: String::new(),
                    sample_rate: 48_000,
                    bit_depth: 16,
                    channels: 2,
                    channel_layout: "stereo".into(),
                    duration_ms: 1_000,
                    byte_length: 100,
                    lossless: false,
                    original: false,
                    streamable: true,
                },
            ],
            publisher_identity_cid: "bafyidentity".into(),
            created_at: 1,
            updated_at: 1,
            publisher_signature: Some("signature".into()),
        }
    }

    #[test]
    fn local_track_persists_and_missing_file_is_marked_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("track.flac");
        std::fs::write(&media, b"audio").unwrap();
        let path = dir.path().join("library.json");
        let service = LibraryService::open(&path).unwrap();
        let imported = service.import_local(track(&media), 1).unwrap();
        std::fs::remove_file(&media).unwrap();
        assert_eq!(service.refresh_availability(2).unwrap(), 1);
        assert_eq!(service.tracks().len(), 1);
        assert_eq!(
            service.track(&imported.track_id).unwrap().scan_state,
            ScanState::Missing
        );
        drop(service);
        assert_eq!(LibraryService::open(path).unwrap().tracks().len(), 1);
    }

    #[test]
    fn source_selection_uses_compatible_rendition_without_transcoding() {
        let dir = tempfile::tempdir().unwrap();
        let service = LibraryService::open(dir.path().join("library.json")).unwrap();
        let imported = service
            .import_manifest("bafymanifest".into(), &manifest(), 1)
            .unwrap();
        let selected = service
            .choose_source(&imported.track_id, &BTreeSet::from(["opus".into()]), true)
            .unwrap();
        assert_eq!(selected.codec, "opus");
        assert!(matches!(
            service.choose_source(&imported.track_id, &BTreeSet::from(["aac".into()]), true),
            Err(LibraryError::NoCompatibleRendition(_))
        ));
    }

    #[test]
    fn playlists_and_stopped_session_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("track.wav");
        std::fs::write(&media, b"audio").unwrap();
        let path = dir.path().join("library.json");
        let service = LibraryService::open(&path).unwrap();
        let imported = service.import_local(track(&media), 1).unwrap();
        let playlist = service.create_playlist("Favorites", 2).unwrap();
        service
            .update_playlist(
                &playlist.playlist_id,
                None,
                Some(vec![imported.track_id.clone()]),
                3,
            )
            .unwrap();
        service
            .save_session(PlaybackSessionV1 {
                current_track_id: Some(imported.track_id.clone()),
                queue: vec![imported.track_id],
                position_seconds: 0.5,
                selected_audio_path: Some("alsa".into()),
                volume: 0.4,
                muted: false,
                auto_play: true,
            })
            .unwrap();
        drop(service);
        let reopened = LibraryService::open(path).unwrap();
        assert_eq!(reopened.playlists()[0].track_ids.len(), 1);
        assert!(!reopened.session().auto_play);
        assert_eq!(reopened.session().position_seconds, 0.5);
    }
}
