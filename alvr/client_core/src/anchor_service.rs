/// T6 — Anchor Responder Service（Quest 端）
///
/// 架构：Pull 模式
///   - Quest 启动时查询/创建锚点，结果缓存到 `AnchorService`
///   - 后台线程监听 UDP :9945，收到任意查询包后回复最新锚点 JSON
///   - UE 插件调用 RequestAnchor(QuestIP) 发送查询，收到响应后触发 Delegate
///
/// 端口：9945（9944 是 ALVR stream_port，避免冲突）
use alvr_common::{Pose, info, warn};
use serde::Serialize;
use std::{
    net::UdpSocket,
    sync::{Arc, Mutex, OnceLock},
    thread,
};

/// 全局单例，供 connection.rs、spatial_anchor.rs（T2）等各处共享。
pub static ANCHOR_SERVICE: OnceLock<AnchorService> = OnceLock::new();

/// 获取全局单例，若未初始化则自动初始化。
pub fn get() -> &'static AnchorService {
    ANCHOR_SERVICE.get_or_init(AnchorService::new)
}

pub const ANCHOR_SERVICE_PORT: u16 = 9945;

// --------------------------------------------------------------------------
// 共享锚点状态
// --------------------------------------------------------------------------

#[derive(Clone)]
pub struct AnchorState {
    pub uuid: String,
    pub pose: Pose,
}

/// 线程安全的锚点缓存。
/// None = 尚未找到锚点；Some = 已定位，可响应查询。
#[derive(Clone)]
pub struct AnchorService {
    state: Arc<Mutex<Option<AnchorState>>>,
}

impl AnchorService {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
        }
    }

    /// 锚点查询/创建成功后调用（T2 实现后接入）
    pub fn update(&self, uuid: String, pose: Pose) {
        info!("AnchorService: anchor updated — uuid={uuid}");
        *self.state.lock().unwrap() = Some(AnchorState { uuid, pose });
    }

    /// 主动放弃锚点（用户选择重新创建时调用）
    pub fn clear(&self) {
        info!("AnchorService: anchor cleared");
        *self.state.lock().unwrap() = None;
    }

    /// 当前是否有有效锚点
    pub fn is_ready(&self) -> bool {
        self.state.lock().unwrap().is_some()
    }

    /// 启动 UDP 响应线程。
    /// 收到任意数据包后，回复最新锚点 JSON（或 not_found）。
    /// 调用一次即可，整个 App 生命周期常驻。
    pub fn start_responder(&self) {
        let state = self.state.clone();

        thread::spawn(move || {
            let socket = match UdpSocket::bind(format!("0.0.0.0:{ANCHOR_SERVICE_PORT}")) {
                Ok(s) => s,
                Err(e) => {
                    warn!("AnchorService: failed to bind UDP :{ANCHOR_SERVICE_PORT} — {e}");
                    return;
                }
            };

            info!("AnchorService: responder ready on UDP :{ANCHOR_SERVICE_PORT}");

            let mut buf = [0u8; 256];
            loop {
                let (_, sender) = match socket.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("AnchorService: recv error: {e}");
                        continue;
                    }
                };

                let json = {
                    let lock = state.lock().unwrap();
                    match &*lock {
                        Some(anchor) => build_response(anchor),
                        None => r#"{"version":1,"status":"not_found"}"#.to_string(),
                    }
                };

                match socket.send_to(json.as_bytes(), sender) {
                    Ok(_) => info!("AnchorService: responded to {sender}"),
                    Err(e) => warn!("AnchorService: send failed to {sender}: {e}"),
                }
            }
        });
    }
}

// --------------------------------------------------------------------------
// JSON 序列化
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
    // 单位约定：输出边界一律转 cm（内部坐标链用米）。旋转四元数无单位。
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
