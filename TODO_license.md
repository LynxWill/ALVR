# 串流授权模块 设计文档（License Gating for ALVR Streamer）

> 目标：让本分支的 **PC 端 ALVR 串流器**只有在本机存在**有效授权文件**时才允许进入串流，
> 并复用现有 VR 应用（LSVR）产生的授权文件；同时把定制头显+定制 PC 锁成封闭生态。
> 本文件是**开工前的方案基线**，实现进度另见末尾「实现顺序」。
>
> 关联：构建/测试工作流见 [`CLAUDE.md`](CLAUDE.md)；空间锚点功能见 [`TODO_spatial_anchor.md`](TODO_spatial_anchor.md)。

---

## 0. 决策快照（已与用户确认）

| 项 | 结论 |
|----|------|
| 验证器实现 | **Rust 原生移植**，且**含激活流程**（串流将来可能脱离 VR 应用独立授权） |
| 授权目录定位 | **ALVR 配置项手填** `Liscene` 绝对路径（不沿用 VR 端相对路径） |
| 放行门槛 | **与 VR 应用对齐**：RSA 验签 + 机器指纹匹配 + 生效/到期判定（Module/30 天联网可后加） |
| PC↔头显分工 | **配对 = `protocol_id` 私有盐分叉**（头显端不加授权逻辑）；**license 全压 PC 端** |
| 第 2 层 HMAC | **不做**（边际收益有限，且会迫使头显端加代码，违背"头显零授权逻辑"） |
| 激活触发方式 | **不在启动时自动激活**（区别于 VR 应用）；改为 dashboard 新增**授权标签页**，手动在线激活 |

---

## 1. 威胁模型与边界（先讲清，避免高估）

- **配对（protocol 分叉）解决"生态隔离"**：谁能和谁连。
  - 定制头显 ↔ stock 同版本 PC ALVR：protocol_id 不符，stock PC 在
    [`connection.rs:552`](alvr/server_core/src/connection.rs:552) 直接拒。
  - stock 头显 ↔ 定制 PC：同样不符，定制 PC 拒。
- **PC license 解决"这台 PC 能否串流"**：机器指纹对不上 → 拒绝串流。
- **配对不解决**：有人同时拷走定制 APK + 定制 PC 构建在未授权 PC 上一起跑——这由 **PC license** 挡（指纹不符）。
- **客户端授权的固有上限**：ALVR 是源码自编译，"无效就拦截"这道判断可被二进制打补丁绕过。
  本方案目标是**挡住普通用户的拷贝/盗用与跨 stock 互通**，不指望防专业破解。两道机制**互补**，不是冗余。

---

## 2. 现有 LSVR 授权系统（信任模型速查）

源码：`Ref/Authorization/LSVR_LicenseSubsystem.{h,cpp}`（UE GameInstanceSubsystem，Windows-only，crypto 走 bcrypt）。

| 环节 | 实现细节（Rust 移植必须逐字对齐） |
|------|----------|
| **授权文件** | `client-license.json` = `{ signature, payload{...}, rawPayloadJson, rawPayloadBase64 }` |
| **磁盘加密** | AES-256-**ECB**（UE `FAES::EncryptData`）+ 自定义 PKCS7 填充（块 16，len%16==0 时补满块）；明文前缀 magic `LSVRLIC1\|`；整体 Base64。<br>密钥 = `SHA256( "LSVR.LocalLicense.2026" + "\|" + MachineFingerprint.大写 )`（32B）。<br>⚠ 解密失败会回退当明文读（legacy 兼容）——Rust 端可不实现回退，但要知道存在。 |
| **签名（信任锚）** | 取 payload 字节（= `rawPayloadBase64` 解码后的 UTF-8 JSON）→ SHA256 → **RSA-2048 PKCS#1 v1.5** 验签。公钥**硬编码内置** `PinnedOfflinePublicKeyXml`（`LSVR_LicenseSubsystem.cpp` 常量）→ **完全可离线验签**。 |
| **机器指纹** | `SHA256_HEX_大写( MachineSeed.Trim.大写 )` |
| **MachineSeed** | 只拼非空项、`\|` 连接，**顺序固定**：<br>`ComputerName={名}` \| `MachineGuid={guid}` \| `SystemDriveSerial={序列}` \| `CpuId={...}` \| `BiosUuid={...}` |
| **机器标识来源** | ① `ComputerName`（`FPlatformProcess::ComputerName()`）<br>② 注册表 `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`<br>③ 系统盘卷序列号 `GetVolumeInformationW(%SystemDrive%)`，格式 `%08X`<br>④ CPUID：leaf0 vendor `%08X%08X%08X`(EBX,EDX,ECX) + leaf1 signature `%08X`(EAX)<br>⑤ SMBIOS `'RSMB'` Type1（System Info）偏移 0x08 的 16B UUID，**前 3 段小端、后 2 段大端**，格式 `%02X..%02X-..-..-..-..` |
| **校验步骤**（`ValidateLocalLicense`） | ① 防时钟回拨（`client-clock.json`，容差 5min）② 激活态存在（`client-state.json`）③ 文件存在 ④ 解密+解析 ⑤ **RSA 验签** ⑥ **指纹匹配**（`payload.MachineFingerprint == 本机`，忽略大小写）⑦ `EffectiveAt`/`ExpiresAt` ISO8601 解析 + 状态判定 ⑧ 每 `MandatoryOnlineValidationDays`(默认 30) 强制联网 |
| **状态机** | Pending（未生效）/ Active / ExpiringSoon（≤3 天）/ GracePeriod（过期但在 GraceDays 内）/ Expired |
| **payload 字段** | `LicenseId, TenantId, CustomerName, ProductCode, Edition, Modules[], LicenseType, MaxDevices, MaxUsers, GraceDays, MandatoryOnlineValidationDays, MachineFingerprint, IssuedAt, EffectiveAt, ExpiresAt, Issuer` |
| **存储目录** | VR 打包版：`<exe>/../../../../../Liscene`（**本项目改为配置项手填绝对路径**）。<br>文件：`client-license.json` / `client-state.json` / `client-clock.json` / `client-display.json` |
| **激活服务** | 固定地址 `http://39.106.116.120:5104`（`FixedActivationServiceBaseUrl`，代码里另有备用注释地址）。 |
| **激活/状态文件加密** | 同 AES 方案，盐/magic 各异：state=`LSVR.LocalState.2026`/`LSVRSTA1\|`，clock=`LSVR.TrustedClock.Local.2026`/`LSVRCLK1\|`。 |

---

## 3. 目标架构

### 3.1 Rust 授权 crate（`alvr/license`，新建）

纯 Rust 移植 LSVR 客户端的**验证 + 激活**子集（不含 UI、不含 UE 依赖）。建议依赖：
`sha2`、`aes`（ECB + 手写 PKCS7）、`rsa` + `rsa::pkcs1v15`、`base64`、`serde`/`serde_json`、
`reqwest`/`ureq`（激活 HTTP）、`windows`（机器标识）、`time`/`chrono`（ISO8601 + 可信时钟）。

模块划分：
- `fingerprint.rs` —— 复刻 §2 的 MachineSeed/指纹（**格式逐字节对齐，最大风险点**）。
- `crypto.rs` —— AES-256-ECB 解/加密 + PKCS7 + magic 校验；RSA PKCS1v1.5-SHA256 验签；内置同一 pinned 公钥。
- `document.rs` —— `client-license.json` 解析（签名 + payload）+ payload 结构体。
- `validate.rs` —— §2 校验步骤；返回与 VR 端一致的状态枚举 + 中文文案。
- `activate.rs` —— 在线激活：`POST` 激活码 + MachineSeed + DeviceName → 收签名授权文档 → 写加密 `client-license.json`/`client-state.json`。
- `clock.rs` —— 可信时钟读/写（防回拨），30 天周期联网判定。
- `store.rs` —— 授权目录定位（读 ALVR 配置项路径）+ 四个文件读写。

> ⚠ **范围**：因为要含激活，这接近**完整 LSVR 客户端的 Rust 复刻**（验签之外还要 HTTP + 写盘 + 时钟）。
> 唯一需要联网对接的是激活协议，schema 见 §6 待确认项。

### 3.2 配对：`protocol_id` 私有盐分叉（只此一层）

- `alvr/common/src/version.rs` 的 `protocol_id()` 输入掺一个**私有盐常量**（只存在于定制构建）。
- 效果：定制 APK 与定制 PC **只能互连**，与任何 stock 同版本 ALVR 互斥（双向，靠现成的
  [`connection.rs:552`](alvr/server_core/src/connection.rs:552) protocol_id 检查）。
- **头显端零授权逻辑**：只是换了对号牌（一个构建常量），不加 license 代码。
- ⚠ 副作用：定制头显**无法**再用任何 stock PC ALVR，定制 PC 也不服务 stock 客户端——这正是需求。

### 3.3 PC 端放行闸门

- **落点**：[`connection.rs:552`](alvr/server_core/src/connection.rs:552) 的 protocol_id 检查旁边——
  license 校验失败时同样 `return Ok(())`（或 `con_bail!`），**在视频流起来之前拦掉**。头显端无感知。
- 校验结果缓存：避免每次握手重算指纹/验签，可在 dashboard 激活后 / 启动时算一次并缓存，
  握手时读缓存 +（廉价的）到期复查。

### 3.4 Dashboard 授权标签页（关键：不自动激活）

- **不在启动时自动激活**（区别于 VR 应用的 `Initialize` 自动校验）。
- 左边栏新增 `Tab::License`（仿 [`mod.rs:48`](alvr/dashboard/src/dashboard/mod.rs:48) 的 `Tab` 枚举 +
  `tab_labels` 注册 + 299–331 分发；组件仿 `dashboard/components/installation.rs` 新建 `license.rs`）。
- 页面内容：
  1. **授权文件路径设置**（`Liscene` 目录绝对路径，文件选择/文本框，存进 ALVR 配置/session）。
  2. **激活码输入框**。
  3. **「在线激活」按钮** —— **仅当①路径已配置 且 ②激活码非空时才可点**（否则禁用置灰）。
  4. 状态显示区：复用 VR 端文案（已激活/未激活/已过期/设备不匹配/网络异常…）+ 到期时间 + Modules。
  5. **允许离线运行的剩余时间**：显示距离"强制联网校验"还能离线多久
     （= `MandatoryOnlineValidationDays`(默认 30) − 距上次联网校验已过天数；取自可信时钟 `client-clock.json`
     的 `LastServerUtc`）。同时若处于宽限期，叠加显示宽限剩余（`ExpiresAt + GraceDays − now`）。
     归零 → 进入"需联网校验"拦截态（与 §2 步骤⑧一致）。这要求 MVP 即启用可信时钟（见 §6）。
- 启动时只做**本地校验并显示状态**（不联网、不自动激活）；联网只发生在用户点「在线激活」时。

---

## 4. 单位 / 格式对齐要点（Rust 端不可偏）

- 指纹大小写：MachineSeed 与最终 hex **都转大写**后再 SHA256 / 比对。
- AES：256-bit key、**ECB**、块 16、PKCS7（注意 len 整除块时补满块那条），密文 Base64。
- 签名输入：是 **payload 原始字节**（`rawPayloadBase64` 解码），不是重新序列化的 JSON——
  务必用文件里的原始字节，避免字段顺序/空白差异导致哈希不符。
- 时间：ISO8601 UTC；状态判定用 UTC now（受可信时钟约束）。

---

## 5. 实现顺序（建议）

1. **黄金测试向量**：取一台真机的真实 `client-license.json` + 它解密后的 payload，作为不漂移基准。
2. `fingerprint.rs` → 跑通：Rust 算出的指纹 == VR 端日志里的指纹（先于一切，否则连解密都不行）。
3. `crypto.rs` + `document.rs` → 用向量验证：能解密、能验签、能解析 payload。
4. `validate.rs` → 离线放行判定（签名+指纹+到期）打通。
5. 接 §3.3 闸门到 `connection.rs:552`，先用本地已有授权文件验证拦/放。
6. §3.2 protocol_id 私有盐分叉（同时改 APK 与 PC 构建常量）。
7. `activate.rs` + `clock.rs` + Dashboard 授权页（§3.4）——激活流程最后做（依赖 §6 协议确认）。

---

## 6. 待确认 / 开放项

- [ ] **激活协议 schema**：从 `SendLicenseRequest` / `HandleLicenseRequestCompleted` /
      `ParseActivationResponse` 扒出请求端点路径、请求体字段、响应体字段（`licenseDocument` 结构），
      Rust 端精确复刻。这是离线验签之外唯一需联网对接的部分。
- [ ] **公钥常量**：把 `PinnedOfflinePublicKeyXml` 的 Modulus/Exponent 提取为 Rust 内置常量。
- [ ] **可信时钟/30 天联网**：**MVP 需启用**（授权页要显示"允许离线运行剩余时间"，依赖
      `client-clock.json` 的 `LastServerUtc` + 周期判定）。需实现 `client-clock.json` 读写 + 防回拨。
- [ ] **Module/ProductCode 门槛**：是否要求 payload 含特定串流权限项？（细粒度授权，可后加。）
- [ ] **私有盐取值**：protocol_id 盐 + （若将来需要）激活 DeviceName 约定。
- [ ] **配置项落点**：授权路径存 ALVR session 还是独立配置文件？（参考锚点用独立
      `anchor_config.json` 的理由：避免频繁改 schema 弄废稳定的 session。）

---

*创建：2026-06-23*
