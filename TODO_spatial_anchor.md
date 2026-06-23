# 空间锚点功能 TODO

## 功能概述

连接 PC 前，在 Lobby 阶段通过**扫描基准码（ArUco fiducial）**确定一个物理空间坐标系（原点 + 朝向），
缓存后对外暴露 UDP 查询服务（T6），PC 上的 UE 插件（T7）按需拉取，用于虚拟内容对齐物理空间。

> **方案演进**：
> - v1（废弃）：连接后推送 / 两点手动放置 + STAGE 坐标 mock
> - v2（废弃）：`XR_EXT_spatial_marker_tracking`（ALVR 内置 marker colocation）。
>   **被 Meta runtime bug 阻断**（见下「v2 失败结论」），应用层无法绕过。
> - v3 Passthrough Camera API 自研 QR（rqrr）：可行但单目 PnP 深度/旋转噪声大。
> - **v3.1（当前）：PCA + ArUco fiducial（`aruco-rs` 纯 Rust）+ 重力约束**。
>   绕开所有 Meta spatial 扩展。ArUco 格子少→远距离更稳；重力约束根治旋转二义性。
>   实机锁间重复性 **~1cm / ~1.6°**。**主 marker 两点法标定「原点相对 marker 的 offset」**，
>   外加 0~N 个辅助 marker（各存「相对主 marker 的 offset」）扩大可定位范围。

### v2 失败结论（XR_EXT marker，已充分验证，留档）

- **v203 / v204（≥v83，功能可用）**：`xrCreateSpatialContextAsyncEXT` 返回 `SUCCESS` +
  无效 future（raw=1）；`poll_future` 和 `create_spatial_context_complete` 均返回
  `ERROR_FUTURE_INVALID_EXT`，runtime 自报 **"Failed to get spatial context future context"**，
  `Fiducials: OFF`。升级 v203→v204 无效。
- **v74（<v83）**：扩展不存在，`QRCodesSpatialContext::new()` 在扩展检查即失败。
- ALVR 代码与上游 master 逐字一致、ABI 正确、权限(USE_SCENE/空间数据 consent)/SpaceSetup 全满足。
- 结论：**Meta 对 XR_EXT_spatial_entity 的 future 实现有 runtime bug**，且标准无 context-complete
  事件可绕过（openxr-sys 已确认）。这是 Meta 主推 XR_FB_*/MRUK、EXT 几乎无人用导致的坑。
- 排查中确认了 spatial-data 运行时 consent 机制（曾在 v203 推进到 context 创建成功）。

---

## v3.1 架构：PCA + ArUco + 主/辅 marker + offset 标定

物理基准用 ArUco fiducial（PCA 取帧检测）。一个**主 marker**定义游戏原点，外加
**0~N 个辅助 marker**扩大可定位范围（大范围活动时任一可见 marker 即可重定位）。
坐标分**配置（一次性）**和**运行**两阶段。

### 配置流程（一次性，配置引导界面，T3/T5）
1. 扫描并保存**主 marker**（id + 尺寸）。
2. 控制器/手势**两点放置**定义游戏原点：第 1 点 = 原点位置，第 2 点 = 原点正方向上一点
   → 由两点生成并保存「**游戏原点相对主 marker 的 offset**」。
3. （可选）逐个添加**辅助 marker**：扫描每张，保存「**该辅助 marker 相对主 marker 的 offset**」。
4. 配置结束，全部落盘（T5）。

### 运行流程（每次串流）
- 串流前要求扫描**主 marker** → `marker 世界 pose × 原点 offset` = 游戏原点 →
  启动 UE 查询服务（T6）→ 进入串流。
- 串流中更新原点：原计划「后台间隔扫描」**因平台限制不可行**（不透明 VR 串流时系统禁用
  前置相机，见 T4「串流期相机限制」），**已撤回**。改用**音量键手势 re-align**（T3.3）：
  手势触发 → 强制开透视 → 重扫任一已知 marker → 重算并更新原点 → 回到串流。
- 锚点存 STAGE 系，摘戴/唤醒/重新点亮自动扛过（T4），日常无需重扫。

```
游戏原点_in_STAGE = marker_in_STAGE × (该marker→主marker offset) × (主marker→原点 offset)
                    主 marker 时第一项 offset = identity
```

- **主/辅 marker**：主 marker 唯一、定义原点；辅助 marker 仅用于扩大覆盖。
- **offset**：原点可任意指定、跨会话复用（marker 贴固定处，原点想放哪放哪）。
- **STAGE**：marker 世界 pose 存 STAGE 系（房间原点持久，扛过重新点亮，见 T4）。
- ⚠ **当前实现**仅做到「扫单张 marker → 其 pose 直接作原点 → 串流前提交」；
  主/辅 marker、两点 offset 标定、配置向导、运行期间隔重扫**均未实现**（见 T3/T4/T5）。

**Camera2 必须走 Java**：Meta 只在 Java 层暴露 passthrough vendor tag
（`com.meta.extra_metadata.camera_source`=0 / `.position` 0左1右），且 openCamera 的
StateCallback 必须 Java 实现。ALVR 纯 Rust，故引入 Java helper（`java/.../CameraHelper.java`）
→ javac+d8 编译为 dex（`scripts/build_camera_helper.ps1`）→ `include_bytes!` 嵌入 .so
→ 运行时 `InMemoryDexClassLoader` 加载 + JNI 调用（jni 0.22 API，参考 `system_info/android.rs`）。

**~~QR payload 格式~~（v3.1 废弃，留档）**：旧 rqrr 版用 `<字母><浮点cm>`（如 `A13.3`）payload
携带尺寸，`parse_qr` 解析，`scripts/gen_anchor_qr.py` 生成。
- **现方案（ArUco DICT_4X4）无 payload 字符串**：尺寸由 **marker id 区间**自带
  （0-19→16cm、50-69→24cm、100-119→34cm、150-169→50cm、200-219→72cm，见 `camera.rs::marker_size_m`）。
  现成可打印码在 `alvr/Aruco/`（PDF，A0-A4），不再用 `gen_anchor_qr.py`。
- 码表 `aruco_dict_4x4.rs`（OpenCV DICT_4X4_250）喂给 aruco-rs 字典无关检测器；
  `disambiguate_corners` 修 aruco-rs 0.1.0 的 180° 角点翻转。

---

## 约定（单位 / 坐标）

- **单位 cm（输出边界）**：凡**显示在 UI** 或**传给 UE** 的 transform 位置数据，单位**一律转 cm**。
  OpenXR / 内部坐标链计算仍用**米**，只在"输出边界"做一次 ×100 转换。旋转用四元数（无单位）。
  - ✅ **已落实（2026-06-05）**：
    - `anchor_ui.rs` `hud_text`/`status_suffix`：经 `to_cm()` 显示 cm（`{:.1}` + " cm"）。
    - `anchor_service.rs` JSON `position`：×100 发 cm；`coordinate_system` = `OpenXR_STAGE_RightHand_Yup_cm`。
    - UE 插件（`AnchorReceiverSubsystem.cpp`）：`Location=FVector(-pz,px,py)` 已**去掉 ×100**（输入即 cm）。
- **坐标系**：OpenXR **STAGE** 右手系、Y-up、−Z 前（房间 guardian 原点）。

---

## 整体流程

```
App 启动
  → [启动界面] 后台查找已保存的主 marker（延迟 core_context.resume()）
        UI: 当前任务 / 已保存 marker 信息 / [重新配置] 按钮
        ├ 用户未点按钮 且 后台定位到主 marker
        │     → 经原点 offset 得游戏原点 → 启动 UE 查询服务(T6) → 进入串流
        └ 用户点 [重新配置]（定位到主 marker 之前）
              → 提示"将清除已保存信息" + [确认]/[取消]
              → 确认 → 打断查找 → 进入【配置引导界面】

  → [配置引导界面]（一次性，T3）
        1. 扫描主 marker → 定位后问确认 [下一步]/[重新扫描] → 下一步保存主 marker
        2. 控制器/手势定位原点（显示射线 + 定位点）→ [下一步]/[重新定位]
        3. 定位原点正方向上一点（显示射线 + 原点点 + 正方向点 + 原点→正方向射线）
              → [下一步]/[重新定位] → 下一步用两点生成 offset 并保存
        4. 问是否继续加辅助 marker [继续添加]/[完成配置]
              → 完成 → 返回启动界面走启动流程；继续 → 第 5 步
        5. 扫描新 marker（已保存的显示"已保存"）→ 新 marker 问确认 [确认]/[重新扫描]
              → 确认 → 保存辅助 marker + 其相对主 marker 的 offset
              → 问是否继续 [继续添加]/[完成配置] → 继续=重复第 5 步 / 完成=返回启动界面

  → [串流] ALVR 默认连接 / 串流流程
        - core_context.resume() 启动连接 + T6 响应服务
        - UE 通过 RequestAnchor 拉取游戏原点(T6/T7)
        - 音量键手势 re-align：扫到主/辅助任一 → 更新游戏原点(T3.3，开透视重扫)
          （原「后台间隔扫描」T4 因平台禁用相机已撤回）
        - marker 世界 pose 存 STAGE 系，摘戴/重定位自动扛过(T4)
```

---

## TODO 列表

### T0｜UI 框架基础设施 ✅（已完成，QR 复用）

> 上一轮为两点放置实现，QR 方案直接复用。

- [x] `graphics/lobby.rs`：`render` 加 `extra_lines` 参数，`line_pipeline` 画任意彩色线段
- [x] `client_openxr/lib.rs`：`AppPhase` 状态机，延迟 `resume()`，阶段 A→B 切换
- [x] `client_openxr/lobby.rs`：采集射线（控制器 aim / 手势腕关节 fallback）+ trigger/pinch
- [x] `client_openxr/anchor_ui.rs`：射线命中检测、确认按钮（实心矩形）、HUD 文字、阶段状态
- [x] anchor_ui 已改为 marker 驱动（不再 mock，见 T3.1）

---

### T1｜~~OpenXR marker 扩展~~（v2 废弃，留档）

> `XR_EXT_spatial_marker_tracking` 路径被 Meta runtime bug 阻断（见「v2 失败结论」）。
> `spatial_marker_tracking.rs` 的调试代码（含 QRDIAG 日志、直接 complete 改动）暂留，
> 后续清理；marker 检测会刷屏 `context_complete res=ERROR_FUTURE_INVALID_EXT`，调相机时需禁用。

- [x] 清理：`lobby_interaction_sources.markers_to_track = None` 关闭 marker 检测（停止刷屏）
- [x] 清理（2026-06-04）：删除 `qr_anchor.rs`（旧 XR_EXT 版，未用）；移除 `spatial_marker_tracking.rs`
      + `interaction.rs` + `lobby.rs` 的 QRDIAG 诊断日志与死 marker 轮询块。
      marker infra（`QRCodesSpatialContext` 等）保留休眠留档，但不再刷屏。

---

### T2｜PCA 基准码检测（`camera.rs` + `CameraHelper.java`）✅

> 用 Passthrough Camera API 取帧 + 检测 + PnP 算 pose。分阶段按风险倒序攻。
> **⚠ 检测器 rqrr(QR)→`aruco-rs`(DICT_4X4) 已切换（2026-06-05）**：下方阶段 2d 的 rqrr/`parse_qr`
> 条目记录为历史里程碑；当前实现见底部「已验证里程碑」DICT_4X4 条与 CLAUDE.md。

#### 阶段 1：Java/dex 集成管道 ✅

- [x] `CameraHelper.java`（dummy `ping()`）→ `build_camera_helper.ps1`(javac+d8)→ dex
- [x] `camera.rs`：`include_bytes!` 嵌 dex + `InMemoryDexClassLoader` 加载 + JNI 调用
- [x] 实机验证：`CameraHelper.ping() => pong-from-java`（**管道打通，最大前置风险解除**）

#### 阶段 2a：相机枚举 + vendor tag 识别 ✅

- [x] `CameraHelper.enumerateCameras(Context)`：列相机 + 读 vendor tag(source/position)
- [x] 加权限 `horizonos.permission.HEADSET_CAMERA` + `android.permission.CAMERA`（Cargo.toml）
- [x] 运行时请求（`alvr_system_info::try_get_permission`），首次弹窗授予
- [x] 实机验证：`count=3 [id=1 src=1][id=50 src=0 pos=0][id=51 src=0 pos=1]`
  - **id=50 = passthrough 左，id=51 = passthrough 右**（source=0）；id=1 = avatar(source=1)
  - vendor tag 用 `CameraCharacteristics.Key<Byte>(name, Byte.class)` 读取成功

#### 阶段 2b：取帧 ✅

- [x] helper：openCamera(id=50 左) + StateCallback + ImageReader(YUV_420_888) + CaptureSession + repeatingRequest
- [x] Y 平面去 rowStride padding → 紧凑灰度缓存（线程安全）
- [x] 实机：`OK id=50 640x360` / 2s 内 103 帧 / 230400 字节灰度 / 时间戳有效 ✅
- [x] 分辨率提到 **1280×1280**（helper 选最大输出尺寸）→ ArUco 远距离稳定检测

#### 阶段 2c：内参 + 外参 ✅

- [x] `getCalibration()`：读 `LENS_INTRINSIC_CALIBRATION` + `SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE`
      + `LENS_POSE_TRANSLATION/ROTATION` + `LENS_DISTORTION`
- [x] 实机：fx=fy=866.15, cx=643.36, cy=641.33（@1280²）；lensT/lensR 读到；dist=null ✅
- [x] stream 已提到 1280×1280 = active array（无裁剪/无缩放），内参直接用（早期 640×360 scale 问题作废）

#### 阶段 2d：QR 解码 ✅ / PnP+坐标 进行中

- [x] `rqrr` 解码 QR + `parse_qr` 校验 `<字母><浮点cm>` 格式，提取字母 + 尺寸 + 4 角
- [x] 实机：payload `A13.3` 解析为 (letter='A', size=13.3cm)，非法格式自动跳过 ✅
      **PCA→QR 路线核心可行性确认,不依赖任何 Meta spatial 扩展**
- [x] 检测移到后台线程（`start_qr_detection`，修启动慢）+ `with_local_frame` 管理 local-ref
- [x] 实机：启动恢复正常 + 后台持续检测 QR（~6fps，4 角实时更新）✅
- [x] 自实现 4 点平面 PnP（homography 分解，`qr_pose.rs`）→ QR 相对相机 pose
- [x] 实机标定：QR 码本身 13.3cm（非 18cm 含白边）+ fx=433(@640 scale)
      → dist 0.86m vs 实测 0.88m，**误差 ~2%** ✅；姿态合理
- [x] 坐标链：`QR_in_stage = head * cam_in_head * QR_in_cam`（`lobby.rs` render 内，
      节流 300ms）。`LATEST_QR_IN_CAM`（camera 线程写）→ 主线程读 + 乘当前 head pose。
      `cam_in_head` = Camera2 LENS_POSE 常量，直接用。
- [x] OpenCV(相机,+Y down) → OpenXR(+Y up) 手性转换（绕 X 翻转 180°，`position=(x,-y,-z)`）
- [x] **实机验证（2026-06-04）**：静止时 world-pos 锁定到 **~1mm**
      （`(-0.387,0.323,0.007)` 持续 11s 抖动 <1mm；Y≈0.32 与上次测试一致）。
      → **坐标链 + cam_in_head + 手性转换全部正确。**
- [x] ⚠ 已知：快速移动时 world-pos 瞬态摆动几十 cm。根因 = 相机取帧→解码延迟
      （~150ms 检测循环 + 管线），`qr_in_cam` 对应的是更早头位姿，render 乘的是当前头位姿。
      → 用稳定门控滤掉（见 2e），不做完整时间戳对齐（hold-still 捕获用例不需要）。

#### 阶段 2e：稳定性 + 输出 ✅（门控）

- [x] **稳定门控**（`lobby.rs`）：滑窗 4 帧 @300ms ≈ 1.2s；同字母 + 位置互相 <1cm
      → `COMMIT` 写 `anchor_service`；否则只打 `settling` 日志不提交。
      滤掉快速运动瞬态，契合"看住 QR 静止捕获一次参考"的用例。
- [x] uuid = `qr-<字母>`（如 `qr-A`）
- [x] **实机验证门控（2026-06-04）**：移动期 world-pos 剧烈摆动 → 全程 `settling` 不提交；
      静止 ~1.2s 收敛后翻转 `COMMIT`，锁定 `(-0.621,0.553,-3.292)` 持续 10s 逐位一致。

---

### T3｜扫描 / 配置 UI（改 `anchor_ui.rs` + `lobby.rs`）

> **目标 = 完整配置向导 + 启动界面**（见上「整体流程」）。下方 T3.1/T3.2 是已建好的
> **扫描 + 稳定门控**积木（当前的单码自动提交流程）；多步向导、两点原点、主/辅 marker、
> 「重新配置」按钮等在 T3.4/T3.5，**均未实现**。

#### T3.1 扫描阶段显示 ✅（2026-06-04，`anchor_ui.rs` 重写为 marker 驱动）

- [x] HUD 操作提示 + 检测状态：未检测「Searching…」/ 检测中「Hold still… settling + 实时坐标」/
      已锁定「[OK] QR 'A' locked + origin 坐标」
- [x] 渲染坐标系可视化：在 QR 世界 pose 处画**三轴**（X 红 / Y 绿 / Z 蓝，`push_axes`，
      `push_thick_line` 加粗便于 VR 可见），随检测实时更新；丢失视野 700ms 后回到搜索态
- [x] 射线可视化（灰色，反馈用，`update(pointers)`）

#### T3.2 自动提交（当前简化流程，积木）✅

> 当前是"扫到 marker → 稳定门控 COMMIT → 直接进串流"。最终会被 T3.4 向导取代
> （向导用显式 [下一步]/[确认] 按钮，不再自动提交）。

- [x] marker 经稳定门控 COMMIT → `anchor_ui.is_ready()=true` → 进串流（门控即"静止 1.2s"确认）。
      `camera::LATEST_QR_IN_CAM` 加时间戳判活性。
- [x] 串流 HUD 末尾附加锚点状态（`status_suffix`：marker 字母 + 坐标）
- [x] 实机验证：三轴朝向贴合物理 marker（重力约束后，墙/地均正确）

#### T3.4 配置引导界面（多步向导）🟡（已实现，待实机验证，2026-06-22）

> 一次性配置，生成并保存主 marker + 原点 offset + 辅助 marker offsets（落盘见 T5）。
> 全在 `anchor_ui.rs` 的状态机里（`Phase::Wizard(Step)`）。

- [x] 步骤状态机（5 步）+ 每步 HUD 文案 + 控制器/手势射线交互
- [x] **步骤 1**：扫描主 marker（稳定门控自动 capture）→ [GREEN]确认/[RED]重扫 → 存主 marker(id+尺寸)
- [x] ✅ 📌 **步骤 1 加提示：先确认地面高度准确**（2026-06-23，待实机看排版）。原点放置（步骤 2）
      用 STAGE `y=0` 平面硬编码作地面，不做实测探测；若 guardian 地板标定有偏差，原点高度会跟着偏。
      `anchor_ui.rs` 步骤 1 HUD 加了一行中英小字号提示（`\u{1}` 前缀），让用户先核对再扫码。
      **仅加提示，未改地面探测实现。**
- [x] **步骤 2**：扳机射线打**地面（STAGE y=0）**落原点位置（显示射线 + 黄色十字）→ 重打可移动 → [GREEN]下一步
- [x] **步骤 3**：第 2 点 = 正方向上一点（显示射线 + 原点十字 + 正方向十字 + 连线）→ [GREEN]下一步
      → 两点生成「主 marker→原点」offset（原点 +Z=指向方向、+Y=up）
- [x] **步骤 4**：[GREEN]继续添加/[RED]完成配置 → 完成 `save()` + 回 Ready
- [x] **步骤 5**：扫新 marker（已存的 HUD 显示"ALREADY saved"、禁确认）→ 新 marker [GREEN]确认/[RED]重扫
      → 存辅助 marker + 「辅助→主」offset → 回步骤 4 循环
- [x] ✅ 实机验证（2026-06-22）：步骤 1-4 正常，重启后扫主码恢复原点正常。
- 📝 已知限制：渲染受限（只有线段+单 HUD）→ 按钮用**颜色编码线框盒**（绿=主/红=次/蓝=三），HUD 文字说明各色含义；
      无逐按钮文字标签。若将来需要真正面板/文字交互再做。
- 📝 设计偏差：placement 用「点地面 + 重点击移动」代替显式「重新定位」按钮（实测更直观，沿用此方案）。
- **改动（2026-06-22）**：
  - 步骤 5 扫到重复码：🟩绿=重扫、🟥红=取消添加（回步骤 4）；新码仍 🟩确认/🟥重扫。
  - HUD **中英双语**：需 CJK 字体（Ubuntu 出不了中文）→ android 嵌 **Noto Sans SC**（OFL，
    `graphics/resources/NotoSansSC-VF.ttf`，仅 android 嵌，PC streamer 仍用 Ubuntu）。
  - 排版（实机反馈）：中英**上下两行**（同行太长会被裁），英文小一号。渲染器 `update_hud_message`
    支持**逐行字号**：行首带控制符 `\u{1}`（`SMALL_LINE_PREFIX`）的行用 `FONT_SIZE_SMALL`，
    且改用 base font 的 `outline_glyph` 以尊重每字号；anchor_ui 用 `bz(zh,en)` 生成双语块。
  - 按钮悬停反馈做强（混白 +内框+填充线），手势/控制器射线指到即明显高亮。
  - **原点朝向修正**：原 `origin_pose_from_points` 建成 +Z=前（差 180° yaw）→ UE 里 X/Y 反；
    改 **OpenXR 约定 -Z=前**（`from_cols(right, Y, -fwd)`）→ UE forward=+X。⚠ 改了 offset 含义，
    旧 `anchor_config.json` 需重新配置。

#### T3.5 启动界面（查找主 marker + 重新配置）🟡（已实现，待实机验证）

- [x] 启动读 `anchor_config::get()`：有配置→`Phase::Startup`（HUD 显示已存 marker 信息 + [BLUE]重新配置）；无→直接进向导
- [x] 后台查找：扫到的 marker id == 已存主 marker → `origin_in_stage()` 算原点 → Ready → resume → 串流
- [x] [BLUE]重新配置 → `Phase::ConfirmReset`（"将清除"）→ [GREEN]确认（`anchor_config::clear()` + 进向导）/[RED]取消
- [x] ✅ 实机验证（2026-06-23）：startup 匹配主 marker 自动进串流、重启后恢复原点、引导流程均正常

#### T3.3 串流中重新对齐（隐藏音量键手势，不放界面按钮）

> 串流界面**不放**重对齐按钮，避免误触/占 UI。用头显音量键快速序列触发。

- [x] 触发手势：音量键 **+ − + − + −**（Up/Down 交替 ≥6 次），每次按键间隔 <1.2s（`realign_gesture::note`）
- [x] 捕获：`android_main` input 循环读 `Keycode::VolumeUp/VolumeDown` 的 `KeyAction::Down`，匹配交替序列
- [x] 跨线程通知：匹配成功 → `static REALIGN_REQUESTED: AtomicBool`；`entry_point` 渲染循环每帧 `swap(false)` 轮询
- [x] 进入「重对齐模式」：**保留 `stream_context`（不断开 PC 连接）**，`lobby.begin_realign()` →
      渲染 lobby/anchor UI（`realign_active` 门控渲染分支）→ 扫到任一已知 marker（主/辅）重算原点
      → 推 `anchor_service` → `is_anchor_ready()` 后退出恢复串流渲染
- [x] ✅ **真机验证通过（2026-06-03）**：Quest 沉浸态下 app 正常收到 VolumeUp/VolumeDown KeyEvent，系统未拦截
- [x] 去重：同 keycode 且间隔 <60ms 视为同一次（每个物理按键重复上报）
- [x] 透视背景：VR 串流会关 passthrough（`StreamingStarted` 里 `passthrough_layer=None`）→ 重对齐时
      lobby 背景黑屏；修：进重对齐时若 passthrough 为 None 则新建，退出时还原（`realign_made_passthrough`）。
- [x] ✅ 实机验证（2026-06-23）：音量键 +−+−+− 序列可**重复触发**（撤回了 T4 期的 note() 折叠重写，
      回到原 dedup 版本即正常），待机唤醒后恢复串流正常。
- 说明：重对齐复用 `Phase::Startup`（扫到任一已知 marker→`origin_in_stage`→Ready）；
  startup 匹配也从「仅主码」放宽到「任一已知 marker」。
- ⚠ **同平台限制**：re-align 只有在透视真正可见时才有相机帧（前置相机随 passthrough 通断电）；
  手势会强制开透视,故 re-align 期间能扫；但不透明串流本身期间相机无帧（见 T4）。

---

### T4｜摘戴 / 重定位补偿 ✅（用 STAGE 解决，2026-06-05）

> Quest 摘戴/唤醒会重定位 `LOCAL_FLOOR`/`LOCAL`，使锚点坐标失效。

- [x] **解法 = 锚点存 `STAGE` 系**（房间级 guardian 原点,重新点亮**不重定位**——实机日志
      确认无 STAGE 的 `ReferenceSpaceChangePending`）。head 用 `locate_views(&stage_space)`,
      gizmo 每帧 `stage_space.locate(&reference_space)` 变换画出。扛过摘戴/唤醒**无需重扫**。
- [x] ❌ **废弃** `pose_in_previous_space` 数学补偿：本机报垃圾值（8-15m，实际只动 ~0.2m），
      套上去把锚点甩飞。别再尝试。
- [x] ✅ **修：待机/唤醒后卡"正在连接"（2026-06-22）**：`SessionState::STOPPING`（摘下待机）会
      `core_context.pause()`，但我们把首次 `resume()` 改成「锚点就绪 + 一次性 `resumed` flag」门控后，
      唤醒的 `READY` 不再 resume → 连接一直暂停。修：`READY` 里 `if resumed { core_context.resume(); }`
      （首次仍由锚点门控，之后每次唤醒都重连）。无需重启/重扫（STAGE + 已存配置保持原点）。
- [x] ❌ **串流期间后台间隔扫描更新原点：平台限制阻断，已撤回（2026-06-23）**
    - **平台结论（实机日志钉死）**：Quest 的前置 RGB 相机（PCA）**只在 passthrough 活动时通电**。
      不透明 VR 串流开始（系统 `PT=0, VrMode=1`）的瞬间,相机回调收到
      **`CameraDevice onError code 3 = ERROR_CAMERA_DISABLED`**（系统设备策略禁用，非掉帧）。
      Meta 官方文档亦明写 *"Passthrough feature must be enabled to access the Passthrough Camera API"*。
      → 不透明串流期间**拿不到任何相机帧**，后台再扫无从谈起。
    - **试过且失败**：在串流投影层（满视场、不透明）下面**垫一个隐藏 FB passthrough 层**想骗系统
      保持相机通电 → 合成器把全遮挡的层 cull / 系统认的是可见 passthrough → **仍报 error 3**。无效。
    - **副作用**：上述「隐藏透视层」实验改了 `StreamingStarted`/`RealTimeConfig` 的 passthrough 生命周期，
      **弄坏了待机唤醒**；连同 T4 重构(`update_anchor_for_stream`/`stream_origin_pushed`)、相机错误重开、
      `MIN_MARKER_PX`、手势 note() 重写一并**全部撤回**到本功能之前的状态（快照见 git 分支 `wip-before-t4-revert`）。
    - **替代方案**：唯一能在「会话中」更新原点的路径是 **T3.3 音量键 re-align**（它强制开透视 → 相机通电）。
      但同一限制下，re-align 也只有在透视真正可见时才有相机帧。
    - ⚠ 若将来 ALVR 以**开启 passthrough**（Blend/RGB/HSV chromakey）方式串流，则 `uses_passthrough()` 为真、
      透视层保留、相机持续通电，后台 re-pin **理论可行**——但本项目用例是不透明串流，故不采用。
    - 📊 **耗电预判（2026-06-23，结论性，不实现）**：耗电大头是「为保活相机必须开 passthrough」本身，
      **不是扫码计算**。① passthrough 通电（双 RGB sensor+ISP 取帧 + 合成器 reproject）≈ +1–3W，
      整机串流 7–12W 基准下**续航缩 15%–30%**，且与扫不扫码无关。② ArUco 检测（CPU、无 SIMD、
      6fps 占满一大核）≈ +0.3–0.8W（3%–8%），**可低占空比压到可忽略**（每 3–5s 突发扫 1–2 帧，
      矫正是慢漂移补偿不需高频）。③ 取帧拷贝/JNI ≈ +0.1–0.3W，噪声级。
      **判断**：若串流**本来就开 passthrough**，后台扫码边际成本仅 #2+#3≈0.4–1W（个位数 %），值得做；
      若**为扫码才被迫开 passthrough**（不透明用例），#1 的 15%–30% 是硬成本且画面变透视混合，
      不划算→仍用 T3.3 音量键 re-align。另注意**热**：相机+passthrough+解码长时间叠加易触发降频，
      可能比掉电更先影响串流流畅度。
    - 保留的中性重构：`Lobby::process_marker`（返回 `AnchorTick::{Throttled,Lost,Marker}`）仍供 render/scan 路径用，
      含相机帧去重（`last_cam_instant`）与 `STABLE_WINDOW`(5s) 老化——对密集扫码无害。

---

### T5｜本地缓存（`client_core/src/anchor_config.rs`）✅（2026-06-05，存储层）

> 存**配置结果**（marker 身份 + offset），**不存世界坐标**——坐标每次扫码现取
> （绕开 STAGE 跨会话漂移）。独立文件 `anchor_config.json`（不混进 `session.json`，
> 避免本模块频繁改 schema 时弄废稳定的 hostname/protocol）。

- [x] `AnchorConfig`：
  - `primary: MarkerRef{ id: u32, size_m }` — 主 marker 身份（**用数字 id**，不是非唯一的 letter）
  - `origin_offset: Pose` — 主 marker → 游戏原点（`origin = primary_in_stage * origin_offset`）
  - `auxiliary: Vec<AuxiliaryMarker{ marker, offset_to_primary: Pose }>`（`primary = aux_in_stage * offset_to_primary`）
- [x] 持久化：`anchor_config.json`（`to_string_pretty`）；惰性加载到 `OnceLock<Mutex<Option<_>>>` 单例
- [x] API：`get()` / `is_configured()` / `save(cfg)` / `clear()`（清内存 + 删文件，供 T3.5 重新配置）
- [x] 链式工具：`AnchorConfig::origin_in_stage(id, marker_in_stage)`（套架构公式算原点）、
      `knows_marker(id)`（T3.4 步骤 5「已保存」判定）、`marker_size(id)`
- [x] ✅ **id 透传（2026-06-22）**：`camera::LATEST_QR_IN_CAM` 改 `(u32 id, f32 size_m, Pose, Instant)`，
      lobby 稳定门控按 id 聚类、`QrStatus{id,size_m,pose,stable}` 传给 anchor_ui；`letter` 仅 HUD 显示用。

---

### T6｜锚点响应服务 ✅（现状 Quest 端）→ 🔄 迁移到 PC 端（方案1，2026-06-23 决议，待实现）

> Pull 模式：缓存锚点，UE 调用 `RequestAnchor` 拉取。

#### 🔄 架构决议（2026-06-23）：anchor 服务从 Quest 端迁到 PC 端（方案1）

> 背景：UE 与 PC ALVR 同机；UE 直连 Quest:9945 需手填 IP，且 USB 串流时 Quest LAN IP 不可路由。
> 评估三方案后采用**方案1**（详见对话/CLAUDE.md）。

- [x] ✅ **头显端改"推"**（2026-06-23，待实机验证）：新增 `ClientControlPacket::AnchorUpdate{uuid,pose}`
      （`alvr_packets`）。`client_core::anchor_service` 改为「最新原点 + dirty 标志」缓存
      （lobby 仍 `update()` 写入）；`client_core/connection.rs` 控制循环 `take_pending()` 检测到变化即
      经控制通道推送，新连接建立时 `mark_dirty()` 重推。**Quest 不再开 9945**。
- [x] ✅ **PC 端接收 + 缓存到文件**（2026-06-23）：`server_core/connection.rs` 收 `AnchorUpdate`
      → `anchor_service::get().update()`；新建 `server_core/src/anchor_service.rs` 缓存 + 落盘
      `config_dir/anchor_cache.json`（米存储，启动时回读，掉电/重启保留）。
- [x] ✅ **PC 端查询服务**（2026-06-23）：响应器搬到 PC，绑 **`127.0.0.1:9945`**，在 `ServerCoreContext::new`
      启动；生命周期绑 streamer 进程（进程退出即关）。响应 JSON 与原一致（cm、`..._cm` 坐标系）。
- [x] ✅ **UE 插件不改**：协议/响应格式不变；只需把目标 IP 从手填改成 `127.0.0.1`（部署约定，非代码改）。
- 落点：`alvr_packets`(新 `AnchorUpdate`) / `client_core`(anchor_service 缓存 + connection 推送) /
      `server_core`(anchor_service 响应器 + lib 启动 + connection 接收)。**4 crate 均 `cargo check` 通过**。
- ⚠ **待实机验证**：定制 APK + 定制 streamer 须**同步重建**（协议加了控制包）；验证 UE 查 127.0.0.1
      能拿到 anchor、re-align 后能更新、重启 streamer 后能从文件回读。

新建 `client_core/src/anchor_service.rs`（现状 Quest 端，迁移后逻辑移至 PC server 端）：

- [x] 端口 **9945**（9944 是 ALVR stream_port，避免冲突）
- [x] `AnchorService` 全局单例（`OnceLock`），线程安全
- [x] `update(uuid, pose)` / `clear()` / `start_responder()`
- [x] 响应协议：收到任意 UDP 包 → 回复当前锚点 JSON（或 `not_found`）
- [x] ⚠ **单位约定（见「约定」）**：`position` ×100 发 cm，`coordinate_system`=`..._cm`（2026-06-05）

响应包格式：

```json
{ "version":1, "status":"ready", "uuid":"...", "coordinate_system":"OpenXR_STAGE_RightHand_Yup",
  "position":{"x":0,"y":0,"z":0}, "orientation":{"x":0,"y":0,"z":0,"w":1} }
{ "version":1, "status":"not_found" }
```

---

### T7｜UE 插件 ✅

插件工程：`UEPlugin/QuestAnchorReceive/`（UE 5.7 源码版）

#### T7.1 数据接收 ✅

- [x] `UGameInstanceSubsystem` + `FRunnableThread` + 10ms 轮询
- [x] `RequestAnchor(QuestIP, TimeoutSeconds)` — 发查询包，Pull 模式
- [x] 三种回调：`OnAnchorReceived`(bIsValid=true ready / false not_found) + `OnAnchorRequestTimeout`
- [x] 代理虚拟网卡导致的重复包：`AtomicSet` 原子去重
- [x] ⚠ **修：Client 模式 RequestAnchor 超时（2026-06-05）**——本地 socket 原绑定固定端口 9945，
      "Play As Client" 下 UE 同进程起多个 GameInstance（server+client），各自实例化 subsystem
      抢绑同端口，Quest 回包被 OS 投递给其中一个（常是非请求方）→ 请求方超时。Standalone 单
      GameInstance 无冲突。**改本地绑 ephemeral 端口 0**（pull 模式只需发查询+收回复，Quest 按
      源端口回包，无需固定本地端口）；`ListenPort` 现仅表 Quest 目标端口。

#### T7.2 坐标系转换 ✅

- [x] Epic OpenXR 惯例：`Location=FVector(-z,x,y)*100`，`Rotation=FQuat(-qz,qx,qy,-qw)`
- [x] ⚠ **单位约定（见「约定」）**：Quest 端已发 cm，已**去掉 `*100`**（`-pz,px,py`），仅轴向换算（2026-06-05）
- [x] ✅ 旋转方向实物验证（2026-06-22）：原点朝向曾差 180° yaw（UE 里 X/Y 反），根因是客户端
      `origin_pose_from_points` 用 +Z=前；改 OpenXR -Z=前后，UE forward=+X 正确（见 T3.4 改动）。

#### T7.3 蓝图接口 ✅

- [x] `OnAnchorReceived` / `OnAnchorRequestTimeout` / `GetLastAnchorTransform` / `IsAnchorValid` / `Get/SetQuestPort`（原 `Get/SetListenPort`，2026-06-05 改名）

---

### T8｜OpenXR 权限（`client_openxr/Cargo.toml`）

- [x] `com.oculus.permission.USE_ANCHOR_API`（已有）
- [x] `com.oculus.permission.USE_SCENE`（已有，marker/scene 需要）

---

### T9｜构建与发布 ✅

#### T9.1 Quest APK（`alvr.client.lbestreaming`）

- [x] 包名 `alvr.client.lbestreaming`，标签/应用名 `LBEStreaming`，与 Meta 商店版共存
      （2026-06-23 由 `alvr.client.lynx`/`ALVR_Lynx` 改名；OpenXR application_name 同步）
- [x] `cargo xtask package-client --ci`（需 `ANDROID_NDK_ROOT`；`Git\usr\bin` 放 PATH 末尾提供 unzip）
- [x] `adb install -r target/distribution/apk/alvr_client_openxr.apk`

#### T9.2 PC Streamer（v21-dev13，协议须与 APK 一致）

- [x] `scripts/prepare_windows_deps.ps1` 准备依赖（绕过 unzip 缺失）
- [x] LLVM 22 + pkg-config-lite（winget），设 `LIBCLANG_PATH`
- [x] VS2022 vcvars64 + `cargo xtask build-streamer --gpl`（`Git\usr\bin` 不能在 PATH 头部）
- [x] 发行包 `build/alvr_streamer_windows.zip`（~110MB，含 FFmpeg GPL）
- [x] 注册 SteamVR 驱动：`%LOCALAPPDATA%/openvr/openvrpaths.vrpath`

**端口**：

| 端口 | 协议 | 用途 |
|------|------|------|
| 9943 | TCP | ALVR 握手 |
| 9944 | UDP | ALVR 视频流（不可占用） |
| 9945 | UDP | 锚点 Pull 服务（**PC 端 `127.0.0.1`**，T6 迁移后） |

---

### T10｜LBE 部署 / 分发 ✅（2026-06-23）

> 面向多场地、多头显、**离线**运营。每场地一台 PC、一份场地专属 `anchor_config`。

- [x] **应用改名 LBEStreaming**：头显包名 `alvr.client.lbestreaming`/标签 LBEStreaming；
      PC 窗口标题 + 侧栏(加宽至 210) + dashboard exe 改名 `LBEStreaming.exe`。
- [x] **PC 默认设置**（仅对新建 session 生效）：Recentering=**Stage**、**自动随 dashboard 开关 SteamVR**、
      Resolution=**Very Low**(width 1536)。
- [x] **配对 = 手动 Trust**（不改代码）：关掉 `auto_trust_clients`(debug 默认 true→自动连一切)，
      在 Devices 页对每台 PC 只 Trust 本工位头显。信任记录存 streamer 目录 `session.json`(便携布局)。
- [x] **anchor_config 外部存储**：`set_storage_dir` 把 `anchor_config.json` 改存 app 外部文件目录
      `/sdcard/Android/data/<pkg>/files/`，使 adb 可 push/pull（私有内部目录非 debuggable 包够不到）。
- [x] **离线 adb**：`platform-tools` 放进 streamer 目录（`local_adb_exe` 命中,ALVR 自身也不再联网下载）。
      运营机零 SDK,整目录离线可用。Quest 开发者模式需一次性联网开启。
- [x] **分发脚本**（streamer 目录 + `scripts/`）：`pull_anchor_config.ps1`(母机→PC)、
      `push_anchor_config.ps1`(PC→所有已装 LBEStreaming 的头显,自动跳过未装)。自带 adb 定位。
- [x] ✅ 实机验证(2026-06-23)：APK 装机、外部目录读写、push/pull 往返、UE 从 `127.0.0.1:9945` 收到 anchor 全通。
- ⚠ **协议变更**：T6 加了 `AnchorUpdate` 控制包 → 定制 APK 与定制 streamer 须**同步重建**。

---

## 文件改动汇总

| 文件 | 类型 | 说明 |
|------|------|------|
| `alvr/graphics/src/lobby.rs` | 修改 ✅ | `render` 加 `extra_lines` 彩色线段 |
| `alvr/client_openxr/src/lib.rs` | 修改 ✅ | 阶段机/延迟resume✅；T4 STAGE✅；串流期后台重扫❌（平台禁用相机，已撤回）|
| `alvr/client_openxr/src/lobby.rs` | 修改 ✅/🔲 | 射线采集 + STAGE 坐标链 + 重力约束 + 门控✅；offset/向导🔲 |
| `alvr/client_openxr/src/anchor_ui.rs` | 修改 ✅/🔲 | UI 框架 + marker 驱动✅；配置向导/启动界面🔲（T3.4/T3.5） |
| `alvr/client_openxr/src/camera.rs` | 新建 ✅ | PCA 桥 + ArUco 检测 + `disambiguate_corners` + PnP（替代废弃的 qr_anchor.rs） |
| `alvr/client_openxr/src/aruco_dict_4x4.rs` | 新建 ✅ | DICT_4X4_250 码表 |
| `alvr/client_openxr/src/qr_pose.rs` | 新建 ✅ | 4 点平面 PnP（homography 分解） |
| `alvr/client_core/src/anchor_service.rs` | 新建 ✅ | T6 迁移后＝头显端「最新原点+dirty」缓存（push-on-change） |
| `alvr/server_core/src/anchor_service.rs` | 新建 ✅ | **PC 端**锚点缓存 + 落盘 `anchor_cache.json` + `127.0.0.1:9945` 响应器（T6 迁移） |
| `alvr/client_core/src/anchor_config.rs` | 新建 ✅ | `AnchorConfig` 持久化 → `anchor_config.json`（**外部存储**，`set_storage_dir`，供 adb 分发） |
| `alvr/packets/src/lib.rs` | 修改 ✅ | `ClientControlPacket::AnchorUpdate{uuid,pose}`（T6 头显→PC 推送） |
| `scripts/pull_anchor_config.ps1` / `push_anchor_config.ps1` | 新建 ✅ | 离线 adb 配置分发（T10） |
| `alvr/client_openxr/Cargo.toml` | 修改 ✅ | 包名 `alvr.client.lbestreaming` |
| `UEPlugin/QuestAnchorReceive/` | 新建 ✅ | UE 插件（Pull + Blueprint API） |
| `alvr/Aruco/` | 资源 ✅ | 现成可打印 DICT_4X4 码（PDF，A0-A4，id 自带尺寸） |
| `build/gen_4x4_codes.py` | 新建 ✅ | 从 OpenCV 提取 DICT_4X4 码表生成 `aruco_dict_4x4.rs` |
| ~~`scripts/gen_anchor_qr.py`~~ | 废弃 | 旧 QR 打印生成（被 `alvr/Aruco/` 取代） |
| `scripts/prepare_windows_deps.ps1` | 新建 ✅ | Windows 构建依赖脚本 |
| `scripts/test_anchor_*.py` | 新建 ✅ | UDP 9945 调试工具 |

---

## 实现顺序建议（QR 方案）

> T2/T3/T4/T5/T6/T7 均已完成（见各 T 与底部里程碑）。剩余/状态：
1. ✅ **T5** → `AnchorConfig` 存储（主 marker + 原点 offset + 辅助 marker offsets）
2. ✅ **T3.4 / T3.5** → 配置引导向导 + 启动界面（实机验证通过 2026-06-23）
3. ❌ **T4 串流期后台重扫** → 平台禁用相机，不可行，已撤回（见 T4 节）
4. ✅ **T3.3** → 音量键 +−+−+− 串流中 re-align（实机验证：手势可重复触发、待机唤醒恢复 2026-06-23）
5. ✅ 实物核对 UE 端坐标系朝向（T7.2，2026-06-22 已修正）

---

## 已验证里程碑

- ✅ **DICT_4X4 + 角点消歧 + STAGE 锚点（2026-06-05）**：
  - 改用用户整套 **OpenCV DICT_4X4_250** 码（`alvr/Aruco/` PDF，id 自带尺寸 16-72cm），
    码表 `aruco_dict_4x4.rs` 喂给 aruco-rs 的字典无关检测器。
  - 修 aruco-rs 0.1.0 **180° 角点标号翻转**（转头触发 marker yaw 翻转）：`disambiguate_corners`
    用 marker 码比对内格 bit 纠正角点顺序，物理正确。主机端旋转扫描 36→0。
  - **锚点改 `STAGE` 系**（持久,重新点亮不重置）→ 扛过摘戴/唤醒无需重扫；gizmo 每帧 STAGE→render
    变换画出。废弃 `pose_in_previous_space` 数学补偿（本机报垃圾值 8-15m）。
- ✅ **ArUco + 重力约束 + 三轴 UI（2026-06-04）**：检测 rqrr→`aruco-rs`；
  重力约束 `gravity_align` 根治旋转翻转；1280² 全分辨率 + 精确内参 + 窗口平均。
  实机锁间重复性 **位置 ~1cm（深度 ±0.6cm）/ 旋转纯 yaw ±1.6°**（QR 版为 ±9.6cm 深度 / ±10°带翻转）。
- ✅ APK `alvr.client.lynx` 与 Meta 商店版共存，正常启动
- ✅ 源码编译 PC Streamer（v21-dev13），与 APK 握手连接成功
- ✅ T6 端到端：Quest↔PC:9945 UDP Pull 通；UE 插件去重/超时/三态回调
- ✅ **T6 迁移 + T10 部署（2026-06-23）**：anchor 服务搬到 PC（头显经控制通道 `AnchorUpdate` 推送 →
  PC 缓存+落盘 `anchor_cache.json` → `127.0.0.1:9945` 响应）；**UE 从 localhost 端到端收到 anchor**。
  LBEStreaming 改名、PC 默认设置、anchor_config 外部存储 + 离线 adb + 分发脚本全部实机验证通过。
- ✅ T0 UI 框架：阶段机、射线（控制器+手势）、实心按钮、坐标系线段渲染
- ✅ PC Streamer 发行包 + QR 打印文件

---

*最后更新：2026-06-23（T6 锚点服务迁到 PC 端 + UE localhost 收取验证通过；LBEStreaming 改名/默认设置/
外部存储/离线 adb 分发脚本 = T10 部署完成。早前：T4 后台重扫撤回；T3.3/T3.4/T3.5 实机通过）*
