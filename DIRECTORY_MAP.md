# wt-media-desktop 目录地图

本文只记录实际存在的目录。定位代码时从本文件出发，禁止全仓库扫描。

**特别标注：业务 Vue 页面源码在 `../wt-media-cloud/web`，不在本仓库。** 本仓库只拥有 Tauri 原生壳；`../.generated/frontend` 是构建产物，不是源码。

## 一、Tauri 应用入口

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/main.rs` | 只剩启动序列：`mod` 声明、配置引导、CSP 注入、Builder 装配、18 个命令的 `generate_handler!`（约 115 行；AC-04 上限 300） | 改客户端生命周期、增删命令 |
| `src-tauri/src/bootstrap.rs` | 配置 → 启动所需物件的引导，以及**必须在 Tauri 构建任何东西之前**完成的 CSP 注入 | 改启动顺序 |
| `src-tauri/src/config.rs` | 配置模式：一个 schema、一条解析路径、两条只往更严方向走的叠加规则（production 忽略整个 `WT_MEDIA_DESKTOP_*` 命名空间） | 改配置键、改优先级 |
| `src-tauri/src/paths.rs` | 配置文件定位：打包版看资源目录，开发树看 crate `resources/`，两条路都以编译进二进制的 `PRODUCTION_TOML` 兜底 | 改配置定位 |
| `src-tauri/src/token.rs` | 每次启动生成的本机运行 token。**无 `Debug`/`Display`/`Serialize`**——这是它作为类型而非 `String` 存在的全部理由 | 改本地鉴权 |
| `src-tauri/src/state.rs` | Tauri 管理的原生状态（绑定凭据、sidecar 句柄）。**不越过 IPC 到 Vue** | 改原生状态 |
| `src-tauri/src/logging/` | Desktop 自己的日志（CHG-057）。`setup.rs` 是**唯一**装配入口（`plan()` 决定一切、`install(plan, secrets)` 只装配；在 `main.rs` 的 CSP 注入与启动摘要之间调用）；`paths.rs` 解目录（Production `~/Library/Logs/WTMedia/Desktop`，Development `<manifest>/.local/logs`）；`rolling.rs` 按日期与 20MB 分档、按天与总量删、单条截断标 `truncate=true original_size=<n>`；`targets.rs` 的 `OWNED_TARGETS` 恰 3 个（`agent.supervisor`/`desktop.startup`/`webview`，外来 target 一律不进文件）；`redact.rs` 是**唯一**脱敏落点（在 sink，记录成形后、离开进程前） | 改日志目录、级别、轮转、脱敏、target |
| `src-tauri/resources/desktop.production.toml` | 编译进二进制的生产配置；`environment`/`agent.*`/`cloud.base_url`/`browser.csp_connect_src`/`http.*_timeout_seconds`/`sidecar.start_timeout_ms`/`development.python_fallback`/`logging.*`（`level = "auto"`、20MB/14 天/100MB——出货值由测试与 `rolling::SHIPPED` 逐字对齐） | 改发布默认值 |
| `src-tauri/tauri.conf.json` | 应用配置：`frontendDist: ../.generated/frontend`、devUrl 5174、构建命令指向 `../wt-media-cloud/web`、`bundle.resources: ["resources/*.toml"]`。**CSP 不在此处**——它由 `bootstrap.rs` 在运行期经 `config_mut()` 注入，使地址来自配置而非字面量 | 改窗口/构建配置 |
| `src-tauri/build.rs`、`src-tauri/gen/`、`src-tauri/icons/` | 构建脚本、生成内容、图标 | 改打包资源 |

## 二、Rust 命令与安全桥

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/commands/` | 暴露给 Vue 的 Tauri 命令（18 个）：`agent.rs`、`bind.rs`、`account.rs`、`profile.rs`、`webview.rs`、`public_config.rs`。`webview.rs` 原名 `logging.rs`（CHG-057 T-16 纯重命名，**命令名 `log_js_error` 与前端调用一个字都没动**），改名是因为它报的是 webview 的 JS 错误，与日志子系统无关 | 改命令接口 |
| `src-tauri/src/dto/` | 按消费方分组的 serde 结构体。**字段名与 serde 属性是契约**：`localAgentService.test.js` 按精确相等断言参数对象 | 改命令载荷 |
| `src-tauri/src/http/` | 两个**分开的**客户端类型：`local_agent.rs`（回环、带运行 token）、`cloud.rs`（地址与凭据都是逐请求事实）。分开是因为可信级别不同——合在一起会让「这次调用带没带本机 token」无法从类型上回答 | 改出站客户端 |
| `src-tauri/src/preflight.rs` | 敏感流程共用的 Cloud 预检与守卫。原先在 account/cookie 各抄一份；错误文案由 `tests::message_parity` 钉住 | 改预检、改错误文案 |
| `src-tauri/src/filesystem/` | 文件/目录选择等受控本地文件能力 | 改文件桥 |
| `src-tauri/src/secure_store/` | OS 安全存储（敏感 Token 不进 localStorage） | 改安全存储 |
| `src-tauri/src/system/` | 系统信息与本地系统能力 | 改系统能力桥 |
| `src-tauri/capabilities/default.json` | Tauri 权限能力声明 | 改权限边界 |

规则：Vue 页面不直接访问 Local Agent 动态端口或 Token（地址经 `get_public_config` 取得）；平台特定行为放 Rust 桥模块，不散落在业务 UI。

`filesystem/`、`secure_store/`、`system/`、`updater/` 目前是 3 行空壳——它们被占位保留，尚无实现，不要把它们当成可读的实现来源。

## 三、Agent Sidecar 生命周期

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/sidecar/mod.rs` | sidecar 的启停。两条 spawn 路径都在这里，且**注入同一组四个环境变量**（`WT_MEDIA_LOCAL_API_HOST`/`_PORT`、`WT_MEDIA_AGENT_RUNTIME_TOKEN`、`WT_MEDIA_AGENT_DATA_DIR`）。返回的标签（`sidecar_started`/`started`/`already_running`/`not_running`）逐字保持，Vue 按它分支 | 改 Sidecar 启停 |
| `src-tauri/src/sidecar/drain.rs` | 保留 sidecar 的输出（原先绑给 `_events` 再不消费，等于全丢）。缓冲仍是**内存环形 200 行**，Agent 的 stdout **不落盘、不轮转**；但它的**去向已经分层**（CHG-057 T-16）：进程存活期间的普通输出**一条记录都不产生**（AC-09 的口径），退出时由 `report_exit` 发**恰一条** `agent.supervisor` 记录并带**末 20 行**尾，读失败（`CommandEvent::Error`）另发一条。那两条记录会落盘、会被轮转、会在 sink 里脱敏——脱敏落点在 `src-tauri/src/logging/redact.rs`，不在本文件 | 改侧车日志处理 |
| `src-tauri/src/local_agent/` | 只余一个纯契约类型 `BoundNodeFacts`（绑定后交给 Vue 的非敏感事实） | 改绑定返回 |
| `src-tauri/binaries/` | Sidecar 二进制组件（当前含 `wt-media-agent-aarch64-apple-darwin`） | 改 Sidecar 集成 |

Agent 内部如何执行浏览器操作或 FFmpeg，不属于本仓库（归 `../wt-media-agent`）。

**两条 spawn 路径的差异是有意的**：打包 sidecar 是发布版唯一可走的路，Python fallback 在 debug 构建的开关之后，release 产物里不可达——客户不需要装系统 Python。

## 四、更新与跨平台打包

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/updater/` | 版本检测、更新 | 改更新逻辑 |
| `scripts/build.sh`、`scripts/dev.sh` | 常规构建与开发 | 改构建流程 |
| `scripts/build-release-macos.sh`、`scripts/package-release-macos.sh` | macOS 发布构建与打包 | 改安装包 |
| `scripts/prepare-release-sidecar.sh` | 发布前准备 Sidecar | 改 Sidecar 集成 |
| `scripts/repair-macos-signing.sh` | macOS 签名修复 | 改签名 |
| `scripts/health.sh`、`health-check.mjs`、`health-dev.mjs` | 健康检查 | 改运行检查 |
| `scripts/start-dev.mjs`、`stop-dev.mjs`、`start.sh`、`stop.sh` | 开发启停 | 改开发流程 |
| `packaging` 相关 | 见 `scripts/README.md` 与发布脚本 | — |

支持范围遵循工程基线：Windows x64、macOS Intel、macOS Apple Silicon。

## 五、契约与测试

| 路径 | 职责 |
|---|---|
| `contracts.lock.json` | 消费的契约版本锁 |
| `tests/` | 发布打包测试（`package-release-macos.test.sh`） |

## 六、Vue 构建产物的来源与集成方式

- 开发：`tauri.conf.json` 的 `beforeDevCommand` 进入 `../../wt-media-cloud/web` 跑 `npm run dev:desktop`（devUrl 5174）。
- 发布：`beforeBuildCommand` 先 `prepare-release-sidecar.sh`，再调 `../wt-media-workspace/scripts/build-desktop.sh` 产出前端到 `../.generated/frontend`，由 `frontendDist` 加载。

改 Desktop 业务页面 → 去 `../wt-media-cloud/web`（`web/src/apps/desktop` 与 `web/src/modules`）。

## 七、禁止扫描区

- `target/`（Rust 构建产物）
- `../.generated/`（前端构建产物快照）
- `src-tauri/gen/`（生成内容，除非任务就是核对生成结果）
