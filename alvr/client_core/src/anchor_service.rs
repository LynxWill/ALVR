/// T6 — Anchor cache（Quest 端）
///
/// 架构（方案1，2026-06-23 起）：anchor 不再由 Quest 直接响应 UE，而是
///   - lobby 建立/更新游戏原点后写入这里（`update`）
///   - `connection.rs` 的控制循环检测到变化（`take_pending`）后，经 ALVR 控制通道
///     `ClientControlPacket::AnchorUpdate` 推给 PC；PC 端缓存 + 在 127.0.0.1:9945 响应 UE。
///
/// 本模块只是「最新原点 + 是否有未推送变化」的线程安全缓存，不再开 UDP 端口。
use alvr_common::{Pose, info};
use std::sync::{Arc, Mutex, OnceLock};

/// 全局单例，供 lobby 写入、connection 读取并推送。
pub static ANCHOR_SERVICE: OnceLock<AnchorService> = OnceLock::new();

/// 获取全局单例，若未初始化则自动初始化。
pub fn get() -> &'static AnchorService {
    ANCHOR_SERVICE.get_or_init(AnchorService::new)
}

#[derive(Clone)]
pub struct AnchorState {
    pub uuid: String,
    pub pose: Pose,
}

struct Inner {
    state: Option<AnchorState>,
    /// True when `state` changed since the last `take_pending`, i.e. there is an
    /// update that still needs to be pushed to the PC.
    dirty: bool,
}

/// 线程安全的最新原点缓存 + 待推送标志。
#[derive(Clone)]
pub struct AnchorService {
    inner: Arc<Mutex<Inner>>,
}

impl Default for AnchorService {
    fn default() -> Self {
        Self::new()
    }
}

impl AnchorService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: None,
                dirty: false,
            })),
        }
    }

    /// 游戏原点建立/更新后调用（lobby）。标记为待推送。
    pub fn update(&self, uuid: String, pose: Pose) {
        info!("AnchorService: anchor updated — uuid={uuid}");
        let mut inner = self.inner.lock().unwrap();
        inner.state = Some(AnchorState { uuid, pose });
        inner.dirty = true;
    }

    /// 主动放弃原点（重新配置时）。
    pub fn clear(&self) {
        info!("AnchorService: anchor cleared");
        let mut inner = self.inner.lock().unwrap();
        inner.state = None;
        inner.dirty = true;
    }

    /// 当前是否有有效原点。
    pub fn is_ready(&self) -> bool {
        self.inner.lock().unwrap().state.is_some()
    }

    /// 标记为待推送（新连接建立时调用，使已有原点重新推给新的 PC）。
    pub fn mark_dirty(&self) {
        self.inner.lock().unwrap().dirty = true;
    }

    /// 若有未推送的原点变化则取出（并清除 dirty）。返回 `Some` 表示需要推送该原点。
    /// 清除态（state=None）不推送，PC 端保留上一次结果。
    pub fn take_pending(&self) -> Option<AnchorState> {
        let mut inner = self.inner.lock().unwrap();
        if inner.dirty {
            inner.dirty = false;
            inner.state.clone()
        } else {
            None
        }
    }
}
