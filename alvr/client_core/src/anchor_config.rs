//! T5 — Anchor configuration persistence (Quest side).
//!
//! Stores the *identity + offsets* that define the game origin relative to the
//! physical ArUco markers — NOT world coordinates. World poses are re-scanned
//! every session (the marker is fixed, so its scanned STAGE pose is consistent),
//! which sidesteps STAGE cross-session drift.
//!
//! Produced by the setup wizard (T3.4):
//!   - one **primary** marker (id + physical size) whose pose, combined with
//!     `origin_offset`, yields the game origin;
//!   - 0..N **auxiliary** markers, each storing its rigid offset *to the primary
//!     marker*, so any single visible marker can recover the origin (T4 re-scan).
//!
//! Coordinate chain (see TODO「v3.1 架构」):
//!   origin_in_STAGE = marker_in_STAGE * (marker→primary offset) * (primary→origin offset)
//! (the marker→primary term is identity when the detected marker IS the primary).
//!
//! Persisted to its own file `anchor_config.json` (separate from `session.json`)
//! so this rapidly-evolving schema can't invalidate the stable client `Config`.

use alvr_common::{Pose, error, info};
use app_dirs2::{AppDataType, AppInfo};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

fn config_path() -> PathBuf {
    app_dirs2::app_root(
        AppDataType::UserConfig,
        &AppInfo {
            name: "ALVR Client",
            author: "ALVR",
        },
    )
    .unwrap()
    .join("anchor_config.json")
}

/// A saved marker's identity. The numeric ArUco `id` is the real key; `letter`
/// elsewhere is only a non-unique display alias (`id % 26`). `size_m` is the
/// physical edge length in metres (encoded by the id range, see
/// `camera::marker_size_m`), kept so PnP can scale without re-deriving it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MarkerRef {
    pub id: u32,
    pub size_m: f32,
}

/// An auxiliary marker plus its rigid offset *to the primary marker*: given the
/// auxiliary's scanned pose `aux_in_stage`, `primary_in_stage = aux_in_stage *
/// offset_to_primary`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct AuxiliaryMarker {
    pub marker: MarkerRef,
    pub offset_to_primary: Pose,
}

/// Full anchor configuration. `origin_offset` is the game origin expressed in the
/// primary marker's frame (from two-point placement): given the primary's scanned
/// pose `primary_in_stage`, `origin_in_stage = primary_in_stage * origin_offset`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AnchorConfig {
    pub primary: MarkerRef,
    pub origin_offset: Pose,
    pub auxiliary: Vec<AuxiliaryMarker>,
}

impl AnchorConfig {
    /// True if `id` is the primary marker or one of the auxiliaries.
    pub fn knows_marker(&self, id: u32) -> bool {
        id == self.primary.id || self.auxiliary.iter().any(|a| a.marker.id == id)
    }

    /// Physical size (metres) saved for `id`, if known.
    pub fn marker_size(&self, id: u32) -> Option<f32> {
        if id == self.primary.id {
            Some(self.primary.size_m)
        } else {
            self.auxiliary
                .iter()
                .find(|a| a.marker.id == id)
                .map(|a| a.marker.size_m)
        }
    }

    /// Given a detected marker's pose in STAGE, compute the game origin in STAGE.
    /// Applies the marker→primary offset (identity for the primary) then the
    /// primary→origin offset. Returns `None` if `id` isn't part of this config.
    pub fn origin_in_stage(&self, id: u32, marker_in_stage: Pose) -> Option<Pose> {
        let marker_to_primary = if id == self.primary.id {
            Pose::IDENTITY
        } else {
            self.auxiliary
                .iter()
                .find(|a| a.marker.id == id)?
                .offset_to_primary
        };
        Some(marker_in_stage * marker_to_primary * self.origin_offset)
    }
}

// --------------------------------------------------------------------------
// Global cache + persistence
// --------------------------------------------------------------------------

/// In-memory copy of the persisted config, lazily loaded from disk on first use.
/// `None` = not configured (wizard hasn't run, or it was cleared).
static STORE: OnceLock<Mutex<Option<AnchorConfig>>> = OnceLock::new();

fn store() -> &'static Mutex<Option<AnchorConfig>> {
    STORE.get_or_init(|| Mutex::new(load_from_disk()))
}

fn load_from_disk() -> Option<AnchorConfig> {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<AnchorConfig>(&s) {
            Ok(cfg) => {
                info!(
                    "AnchorConfig: loaded — primary id={} ({:.0}cm), {} auxiliary",
                    cfg.primary.id,
                    cfg.primary.size_m * 100.0,
                    cfg.auxiliary.len()
                );
                Some(cfg)
            }
            Err(e) => {
                // Schema changed or file corrupt — treat as unconfigured rather
                // than crashing; the wizard can rewrite it.
                info!("AnchorConfig: parse failed ({e}); treating as unconfigured");
                None
            }
        },
        Err(_) => None, // no file yet = not configured
    }
}

/// Current configuration (clone), or `None` if not configured.
pub fn get() -> Option<AnchorConfig> {
    store().lock().unwrap().clone()
}

/// Whether a primary marker has been configured.
pub fn is_configured() -> bool {
    store().lock().unwrap().is_some()
}

/// Save a new configuration (overwrites any existing) to memory + disk.
pub fn save(config: AnchorConfig) {
    info!(
        "AnchorConfig: saved — primary id={} ({:.0}cm), {} auxiliary",
        config.primary.id,
        config.primary.size_m * 100.0,
        config.auxiliary.len()
    );
    match serde_json::to_string_pretty(&config) {
        Ok(s) => {
            if let Err(e) = fs::write(config_path(), s) {
                error!("AnchorConfig: write failed: {e}");
            }
        }
        Err(e) => error!("AnchorConfig: serialize failed: {e}"),
    }
    *store().lock().unwrap() = Some(config);
}

/// Forget the saved configuration (the「重新配置」path, T3.5): clears memory and
/// deletes the file.
pub fn clear() {
    info!("AnchorConfig: cleared");
    *store().lock().unwrap() = None;
    let path = config_path();
    if path.exists() {
        if let Err(e) = fs::remove_file(&path) {
            error!("AnchorConfig: delete failed: {e}");
        }
    }
}
