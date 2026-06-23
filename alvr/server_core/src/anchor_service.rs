//! T6 — Anchor responder service (PC side).
//!
//! Scheme 1 (2026-06-23): the headset pushes the established game origin over the
//! ALVR control channel (`ClientControlPacket::AnchorUpdate`). The server caches it
//! here, persists it to a file (survives restart), and answers UE's pull queries on
//! `127.0.0.1:9945`. UE always targets localhost — no per-station IP config, works
//! over both Wi-Fi and USB streaming.
//!
//! Lifecycle: the responder thread lives for the streamer process and dies with it.

use alvr_common::{
    Pose, info, warn,
    glam::{Quat, Vec3},
};
use serde::{Deserialize, Serialize};
use std::{
    net::UdpSocket,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    thread,
};

pub const ANCHOR_SERVICE_PORT: u16 = 9945;

static ANCHOR_SERVICE: OnceLock<AnchorService> = OnceLock::new();

/// Global singleton. Initialized by `start` (with the cache file path); `get`
/// returns a no-file fallback if `start` was never called.
pub fn get() -> &'static AnchorService {
    ANCHOR_SERVICE.get_or_init(|| AnchorService::new(None))
}

/// Initialize the singleton with the cache file path, load any persisted anchor,
/// and start the UDP responder. Call once at server startup.
pub fn start(cache_path: PathBuf) {
    let service = ANCHOR_SERVICE.get_or_init(|| AnchorService::new(Some(cache_path)));
    service.start_responder();
}

#[derive(Clone)]
pub struct AnchorState {
    pub uuid: String,
    pub pose: Pose,
}

pub struct AnchorService {
    state: Arc<Mutex<Option<AnchorState>>>,
    cache_path: Option<PathBuf>,
    responder_started: Arc<Mutex<bool>>,
}

impl AnchorService {
    fn new(cache_path: Option<PathBuf>) -> Self {
        let state = cache_path
            .as_ref()
            .and_then(|p| load_from_file(p))
            .map(|s| {
                info!("AnchorService: loaded persisted anchor — uuid={}", s.uuid);
                s
            });

        Self {
            state: Arc::new(Mutex::new(state)),
            cache_path,
            responder_started: Arc::new(Mutex::new(false)),
        }
    }

    /// Called when an `AnchorUpdate` arrives from the headset. Caches + persists.
    pub fn update(&self, uuid: String, pose: Pose) {
        info!("AnchorService: anchor updated from headset — uuid={uuid}");
        if let Some(path) = &self.cache_path {
            save_to_file(path, &AnchorState { uuid: uuid.clone(), pose });
        }
        *self.state.lock().unwrap() = Some(AnchorState { uuid, pose });
    }

    pub fn clear(&self) {
        info!("AnchorService: anchor cleared");
        *self.state.lock().unwrap() = None;
    }

    pub fn is_ready(&self) -> bool {
        self.state.lock().unwrap().is_some()
    }

    /// Start the UDP responder on `127.0.0.1:9945` (idempotent).
    pub fn start_responder(&self) {
        {
            let mut started = self.responder_started.lock().unwrap();
            if *started {
                return;
            }
            *started = true;
        }

        let state = self.state.clone();

        thread::spawn(move || {
            let socket = match UdpSocket::bind(format!("127.0.0.1:{ANCHOR_SERVICE_PORT}")) {
                Ok(s) => s,
                Err(e) => {
                    warn!("AnchorService: failed to bind UDP 127.0.0.1:{ANCHOR_SERVICE_PORT} — {e}");
                    return;
                }
            };

            info!("AnchorService: responder ready on UDP 127.0.0.1:{ANCHOR_SERVICE_PORT}");

            let mut buf = [0u8; 256];
            loop {
                let sender = match socket.recv_from(&mut buf) {
                    Ok((_, sender)) => sender,
                    Err(e) => {
                        warn!("AnchorService: recv error: {e}");
                        continue;
                    }
                };

                let json = match &*state.lock().unwrap() {
                    Some(anchor) => build_response(anchor),
                    None => r#"{"version":1,"status":"not_found"}"#.to_string(),
                };

                if let Err(e) = socket.send_to(json.as_bytes(), sender) {
                    warn!("AnchorService: send failed to {sender}: {e}");
                }
            }
        });
    }
}

// --------------------------------------------------------------------------
// File persistence (stored in metres, internal units)
// --------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct PersistedAnchor {
    uuid: String,
    px: f32,
    py: f32,
    pz: f32,
    qx: f32,
    qy: f32,
    qz: f32,
    qw: f32,
}

fn load_from_file(path: &PathBuf) -> Option<AnchorState> {
    let text = std::fs::read_to_string(path).ok()?;
    let p: PersistedAnchor = serde_json::from_str(&text).ok()?;
    Some(AnchorState {
        uuid: p.uuid,
        pose: Pose {
            position: Vec3::new(p.px, p.py, p.pz),
            orientation: Quat::from_xyzw(p.qx, p.qy, p.qz, p.qw),
        },
    })
}

fn save_to_file(path: &PathBuf, anchor: &AnchorState) {
    let p = PersistedAnchor {
        uuid: anchor.uuid.clone(),
        px: anchor.pose.position.x,
        py: anchor.pose.position.y,
        pz: anchor.pose.position.z,
        qx: anchor.pose.orientation.x,
        qy: anchor.pose.orientation.y,
        qz: anchor.pose.orientation.z,
        qw: anchor.pose.orientation.w,
    };
    match serde_json::to_string_pretty(&p) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                warn!("AnchorService: failed to persist anchor to {path:?}: {e}");
            }
        }
        Err(e) => warn!("AnchorService: serialize for persist failed: {e}"),
    }
}

// --------------------------------------------------------------------------
// UE response JSON (positions converted to cm at the output boundary)
// --------------------------------------------------------------------------

#[derive(Serialize)]
struct PositionData {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Serialize)]
struct OrientationData {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

#[derive(Serialize)]
struct AnchorResponse<'a> {
    version: u32,
    status: &'static str,
    uuid: &'a str,
    coordinate_system: &'static str,
    position: PositionData,
    orientation: OrientationData,
}

fn build_response(anchor: &AnchorState) -> String {
    // Unit convention: convert to cm at the output boundary (chain is in metres).
    const M_TO_CM: f32 = 100.0;
    let resp = AnchorResponse {
        version: 1,
        status: "ready",
        uuid: &anchor.uuid,
        coordinate_system: "OpenXR_STAGE_RightHand_Yup_cm",
        position: PositionData {
            x: anchor.pose.position.x * M_TO_CM,
            y: anchor.pose.position.y * M_TO_CM,
            z: anchor.pose.position.z * M_TO_CM,
        },
        orientation: OrientationData {
            x: anchor.pose.orientation.x,
            y: anchor.pose.orientation.y,
            z: anchor.pose.orientation.z,
            w: anchor.pose.orientation.w,
        },
    };
    serde_json::to_string(&resp).unwrap_or_else(|e| {
        warn!("AnchorService: serialize failed: {e}");
        r#"{"version":1,"status":"error"}"#.to_string()
    })
}
