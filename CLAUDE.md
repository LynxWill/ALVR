# CLAUDE.md — ALVR SpatialAnchor 分支工作笔记

PC→Quest VR 串流（Rust）上自研的**空间锚点 co-location**功能。详细 TODO 见
[`TODO_spatial_anchor.md`](TODO_spatial_anchor.md)，本文件只放**构建/测试工作流**和**架构速查**。
串流**授权/加密 license** 模块设计见 [`TODO_license.md`](TODO_license.md)（复用 VR 应用 LSVR 授权文件
+ protocol_id 私有盐配对 + dashboard 授权页，方案基线已定、待开工）。

- 分支：`SpatialAnchor`（主干 `master`）
- 平台：Windows 11 + PowerShell；Quest 3/3S（HorizonOS v74+，实测 v204）

---

## 工作风格（本仓库协作约定，沿用）

> 这套风格在本项目实践中行之有效，后续会话照此协作。

- **先查证再下结论**：读实际代码 / 文件 / 设备状态来支撑回答，**不靠记忆猜**（路径、握手逻辑、
  默认值、存储位置一律实地核实）。平台/事实性论断（Quest STAGE、USB OTG 等）**用户要核实时上网查**并给来源。
- **决策导向**：给**推荐**而非罗列；多方案用**对比表**，明说"我推荐 X，因为 Y"；
  **区分有把握 vs 不确定**，诚实标 caveat，不夸大（"已验证"才说验证，撤回的实验留档防重蹈）。
- **改完即验证**：`cargo check`(相关 crate，含 `--target aarch64-linux-android`) → 构建 → 装机 →
  **端到端往返/冒烟测试**；临时测试产物（设备上的 / 本地的）**用完即清理**。
- **结构化中文**：表格 + 分点 + 关键结论加粗；UI 文案中英双语；文件引用用可点击 `path:line`。
- **主动暴露影响与待办**：显式标注"需实机验证""协议变更须两端同步重建""还没提交"；
  **改大动作前先 commit 检查点**（教训见下「改坏过的教训」）。
- **长构建后台跑**（`run_in_background`），**等通知不轮询**；早期瞄一眼输出catch 立即失败。
- **破坏性 / 对外操作先确认**；**安全/正确性把关**：不提交他人专有代码（`Ref/` 已 gitignore）、
  不外泄敏感信息；离线 / 部署约束（如 30 天联网校验、adb 离线）主动提示。

---

## 构建工作流

### Quest 客户端 APK（`alvr.client.lynx`，标签 ALVR_Lynx，与商店版共存）

```powershell
# Git 的 unzip 放 PATH 末尾（不能放头部：其 link.exe 会顶替 MSVC linker）
$env:PATH = "$env:PATH;C:\Program Files\Git\usr\bin"
cargo xtask package-client --ci          # --ci 跳过 choco；需 ANDROID_NDK_ROOT
& "$env:ANDROID_HOME\platform-tools\adb.exe" install -r `
    target\distribution\apk\alvr_client_openxr.apk
```

- 工具链：NDK 28.2.13676358、SDK platform-32、build-tools 37（d8）、JDK 21
- 快速语法检查（不打包）：`cargo check -p alvr_client_openxr --target aarch64-linux-android`

### Camera2 Java helper → dex（**仅当改了 `CameraHelper.java` 时**）

```powershell
scripts\build_camera_helper.ps1   # javac(-source/target 8) + d8(--min-api 26)
                                  # → assets/camera_helper.dex（被 include_bytes! 嵌入 .so）
```

### PC Streamer（源码版，协议须与 APK 一致，当前 v21-dev13）

```powershell
scripts\prepare_windows_deps.ps1                       # 绕过 unzip 缺失
# 设 LIBCLANG_PATH(LLVM)、跑 VS2022 vcvars64
cargo xtask build-streamer --gpl                        # Git\usr\bin 不能在 PATH 头部
# 产物 build\alvr_streamer_windows.zip；驱动注册见 openvrpaths.vrpath
```

---

## 测试 / 调试工作流（Quest）

```powershell
$adb = "$env:ANDROID_HOME\platform-tools\adb.exe"
& $adb logcat -c                                        # 测前清缓冲（重要）
# ... 在头显里操作 ...
& $adb logcat -d -t 600 2>$null | Select-String "NATIVE-RUST" |
    Select-String "PnP|chain" | Select-Object -Last 30
```

**坑（必看）：**
- **logcat 缓冲 32M**：长时间测试后数据会被刷掉；用 `-t <N>` 限制读取行数，
  **不要** `logcat -d` 全量 dump（会卡住/超时）。测前务必 `logcat -c`。
- **设备 unauthorized**：移动测试中 USB 抖动会丢授权 → logcat 卡死。
  修复：`adb kill-server; adb start-server`，头显里重新"允许 USB 调试"。
- Rust 日志统一 tag `[ALVR NATIVE-RUST]`；关键字 `camera: [aruco]`、`lobby: [chain]`。
  Java 相机日志 tag = `ALVR-CameraHelper`（相机 onError/onDisconnected/startPassthrough 在这里）。
- 调相机时 marker 检测已禁用（`markers_to_track=None`），避免 QRDIAG 刷屏挤掉相机日志。
- **串流期间抓 log 必看**：串流时 `VideoRenderQualityTracker`（系统进程）刷 ~6000 行/秒,
  会把我们的行从 logd 环形缓冲挤掉,**事后 `logcat -d` 抓不到**。两个可靠办法：
  - **按 app PID 过滤**（首选）：`adb logcat --pid=$(adb shell pidof alvr.client.lynx)`——
    NATIVE-RUST 和 CameraHelper 都来自 app 进程,系统刷屏天然排除,再 `grep` 关键字即可。
  - ⚠ **不能用 `logcat -s "[ALVR NATIVE-RUST]"`**:tag 带方括号+空格,白名单匹配不上(只会漏掉),
    曾因此误判"日志没产生"。要么用 `--pid`,要么 `grep` tag 字符串。
  - 边测边抓:`adb logcat -c` 后用 `run_in_background` 起 `adb logcat --pid=… | grep --line-buffered …`。

---

## 架构速查

### 方案演进（细节见 TODO）
- v1 推送/两点放置（废弃）→ v2 `XR_EXT_spatial_marker_tracking`（**Meta runtime bug 阻断,废弃**）
- v3 Passthrough Camera API 自研 QR（rqrr）→ **v3.1（当前）：PCA + ArUco fiducial**。
  绕开所有 Meta spatial 扩展。

### 坐标链 + 参考系（已实机验证正确）
```
marker_in_STAGE = head_in_STAGE * cam_in_head * marker_in_cam
```
- **参考系必须用 `STAGE`,不能用 `LOCAL_FLOOR`**（关键，见下「重新点亮」）。head 用
  `locate_views(&stage_space)` 取两眼中点；lobby 渲染仍用 LOCAL_FLOOR，gizmo 每帧用
  `stage_space.locate(&reference_space)` 的 STAGE→render 变换画出。
- `marker_in_cam`：camera 线程 ArUco 检测 + `solve_qr_pose`(homography PnP) 算出，
  写入全局 `camera::LATEST_QR_IN_CAM`（`Option<(u32 id, f32 size_m, Pose, Instant)>`；Instant 判活性 700ms）
- `cam_in_head`：Camera2 `LENS_POSE`；**注意旋转要 `(flip_x * lens_rot).inverse()`**
  （raw lens 是 device→optical，约 180°-X；qr_in_cam 已在 camera.rs 转过 OpenXR 相机系，
  故这里要抵消 180° 再取逆，否则 QR 落到头后下方/偏上 25cm）。位置直接用。
- 手性：OpenCV(+Y down) → OpenXR(+Y up)，绕 X 翻 180°，`position=(x,-y,-z)`
- **单位约定**：内部坐标链用**米**；凡**显示在 UI** 或**传给 UE** 的位置 transform，
  在输出边界**一律 ×100 转 cm**（`anchor_ui` HUD、`anchor_service` JSON；UE 端随之去掉自己的 `*100`）。

### 重新点亮 / 摘戴 / 唤醒（坐标稳定的根本）
- Quest 摘戴/唤醒会重定位 **`LOCAL_FLOOR`/`LOCAL`**（发 `ReferenceSpaceChangePending`），
  但 **`STAGE`（guardian 房间原点）不动**（实机日志确认无 STAGE 事件）。故锚点存 STAGE 即可
  扛过重新点亮,**无需回到 marker 重扫**（大范围活动时这点关键）。
- ⚠ **`pose_in_previous_space` 在本机是垃圾值**（报 8–15m，实际只动 ~0.2m）——数学补偿这条路
  **废弃**，别再尝试用它变换锚点（会把锚点甩飞）。靠 STAGE 持久性，不靠事件补偿。

### ⛔ 平台限制：前置相机随 passthrough 通断电（PCA 核心约束）
- **PCA 前置 RGB 相机只在 passthrough 活动时通电**。不透明 VR 串流一开始（系统 `PT=0, VrMode=1`），
  相机回调立刻收到 **`CameraDevice onError code 3 = ERROR_CAMERA_DISABLED`**(系统设备策略禁用,**非掉帧**),
  之后整个不透明串流期间**零相机帧**。扫码阶段、re-align 阶段(透视可见)相机才工作。
  Meta 文档亦明写 *"Passthrough feature must be enabled to access the Passthrough Camera API"*。
- **推论**：① 「串流期间后台间隔扫描更新原点」**不可行**(相机没帧),已撤回(原 T4)。会话中更新原点
  只能靠 **T3.3 音量键 re-align**(强制开透视→相机通电)。② 若改用**开 passthrough 的串流**
  (Blend/RGB/HSV chromakey,`uses_passthrough()` 为真→透视层保留→相机持续通电),后台 re-pin 理论可行。
- **已试且失败,别再走**:在不透明串流投影层(满视场、`CompositionLayerFlags::EMPTY` 全不透明)**下面垫一个
  隐藏 FB passthrough 层**想骗系统保持相机通电 → 合成器 cull 全遮挡层 / 系统认可见 passthrough → **仍 error 3**。
- **相机无自动重开**:`CameraHelper` 的 `startPassthrough` 只在检测线程启动时调一次,`onError` 后 `sDevice=null`
  不会复活(本版本如此)。re-align 在相机已被禁用后能否扫到,取决于透视是否真的把相机重新喂上。

### 🧨 改坏过的教训（2026-06-23 大撤回，留档防重蹈）
- **别动 `StreamingStarted`/`RealTimeConfig` 的 passthrough_layer 生命周期来"保活相机"**:那次实验
  (隐藏透视层 + 串流期保留透视层)**弄坏了待机唤醒**(摘戴后卡"正在连接"),且根本没保住相机。
- **`realign_gesture::note()` 用原 dedup 版**(同键 <60ms 判重复)。曾改成"折叠连续同键只数交替"想更鲁棒,
  反而表现为**只生效一次**;回退到 dedup 版后手势可重复触发。别再重写。
- 一次失败实验会牵连多文件(lib/lobby/camera/java),**改大动作前先 `git` 存快照分支/提交**——当时
  全部工作未提交、无检查点,只能临时存 `wip-before-t4-revert` 干净回退。现已提交(`SpatialAnchor`
  分支,PR #1),后续大改动前照例先 commit 一个检查点。

### 检测：ArUco（`aruco-rs` 纯 Rust，喂 **OpenCV DICT_4X4_250** 码表）
- aruco-rs 只内置 5×5/6×6 字典；用户的码是 **OpenCV `DICT_4X4`**（4×4 数据+1 格边=6×6）。
  aruco-rs 检测器字典无关：把 DICT_4X4 码表（`aruco_dict_4x4.rs`，前 250 个=DICT_4X4_250，
  16bit/MSB=内格左上/行优先/1=白）做成 `DictionaryConfig{n_bits:16,tau:3}` 喂进去即可。
- `ScalarCV`（aarch64 不能用 x86 SIMD）。`detect()` 要 **RGBA 输入**：Y 平面扩成 R=G=B=Y,A=255。
- **id→尺寸**（`marker_size_m`）：码按纸张分组,id 自带物理尺寸——0-19→16cm、50-69→24cm、
  100-119→34cm、150-169→50cm、200-219→72cm（`alvr/Aruco/` 整套）。其它 id 忽略（防误检）。
- **180° 角点翻转 bug**（aruco-rs 0.1.0）：非 90°整数倍旋转时角点标号会翻 180°（对角对调），
  转头就触发 → marker yaw 翻转。`solve_qr_pose` 合成测试全 360° 正确,**确认是角点标号问题**。
  修法 `disambiguate_corners`：拿 4 角点采样内格 bit,与 `DICT_4X4_250_CODES[id]` 比对,
  若 180° 反读更近就把角点对调。物理正确（对齐 marker 真实印刷朝向）。主机端旋转扫描 36→0。
- 现成可打印码：`alvr/Aruco/`（PDF，A0-A4 各尺寸）。`scripts/gen_anchor_marker.py` 是早期
  Original-ArUco 生成器（已被 DICT_4X4 方案取代,留作参考）。

### 精度优化（已实机验证，2026-06-04）
锁间重复性 **~1cm 位置 / ~1.6° 旋转**（较 QR 版深度 ±9.6cm→±0.6cm，旋转 ±10°带翻转→纯yaw±1.6°）：
1. **分辨率 1280×1280**（相机最大；selectorpick largest）+ **精确内参** cx,cy=643.4,641.3（stream=active array，无裁剪）。
2. **重力约束**（`gravity_align`，lobby.rs）：STAGE 的 Y 轴=重力，用已知 up 重建朝向——
   墙(法线水平)保 yaw 强制 up=+Y；地(法线≈±Y)保 heading。**根治平面 PnP 的俯仰/横滚二义性翻转**。
   前提：marker 装正（墙竖直/地水平）。
3. **窗口平均**：COMMIT 前对 4 帧稳定窗口取均值（位置算术平均 + 四元数符号对齐 nlerp）。

### 稳定门控（`lobby.rs`）
滑窗 4 帧 @300ms（≈1.2s 静止）；同 id + 位置互相 <1cm → `COMMIT`（写 anchor_service，
打 `LOCK` 边沿日志一次）；否则 `settling`。短轴=实时检测、长轴=锁定结果(冻结)，便于核对重复性。

### 距离 vs 尺寸（实测可用分辨率上限 1280²，fx≈866）
- 码成像像素 = `尺寸×866/距离`；分辨率已到顶。**远距离换大码**——正是 `alvr/Aruco/` 按
  A4(16cm)→A0(72cm) 分组的用意：近距离用小码、远距离用大码。DICT_4X4 格子少（6×6），
  同尺寸比 QR/5×5 检测更远。

### Java↔Rust 桥（PCA 必须走 Java）
Meta 只在 Java 层暴露 passthrough vendor tag（`com.meta.extra_metadata.camera_source`=0,
`.position` 0左1右）。`CameraHelper.java` → d8 → dex → `InMemoryDexClassLoader`(API26+) → JNI。
jni 0.22 callback 式 API，参考 `alvr/system_info/src/android.rs`。
passthrough 相机：**id=50 左 / id=51 右**（source=0）。

### 关键文件
| 文件 | 作用 |
|------|------|
| `alvr/client_openxr/src/camera.rs` | PCA 桥 + ArUco 检测 + `disambiguate_corners` + `marker_size_m` + PnP 调用 |
| `alvr/client_openxr/src/aruco_dict_4x4.rs` | DICT_4X4_250 码表（`gen_4x4_codes.py` 从 OpenCV 提取） |
| `alvr/client_openxr/src/qr_pose.rs` | 4 点平面 PnP（homography 分解 + 重投影消歧） |
| `alvr/client_openxr/src/anchor_ui.rs` | 扫描 HUD + 三轴可视化（STAGE pose,每帧 STAGE→render 变换画出） |
| `alvr/client_openxr/java/.../CameraHelper.java` | Camera2 取帧（选最大分辨率）+ vendor tag + 内外参 |
| `alvr/client_openxr/src/lobby.rs` | STAGE 坐标链 + `gravity_align` + 窗口平均 + 稳定门控 |
| `alvr/client_core/src/anchor_service.rs` | UDP Pull 服务（端口 9945） |
| `alvr/Aruco/` | 现成可打印 DICT_4X4 码（PDF，A0-A4，id 自带尺寸） |
| `build/gen_4x4_codes.py` | 从 OpenCV 提取 DICT_4X4 码表生成 `aruco_dict_4x4.rs`（需 cv2） |
| `UEPlugin/QuestAnchorReceive/` | UE5.7 插件（Pull + Blueprint API） |

### 端口
| 端口 | 用途 |
|------|------|
| 9943 TCP | ALVR 握手 |
| 9944 UDP | ALVR 视频流（**不可占用**） |
| 9945 UDP | 锚点 Pull 服务（本项目） |

---

*最后更新：2026-06-05*
