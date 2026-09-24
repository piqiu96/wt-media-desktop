# wt-media-desktop Agent Index

## 依赖

关于文档等事实都在`../wt-media-workspace`，需要执行时优先考虑对应的约束边界

## 定位

本仓库是 WT Media 的 **Tauri 2 原生客户端工程**：Windows 与 macOS 客户端的原生能力、运行环境、进程生命周期与打包交付。

Desktop 是**客户端控制壳，不是第二套业务系统**。

## 本仓库拥有

- 客户端生命周期：Tauri 初始化、窗口、应用退出、系统托盘、运行状态（`src-tauri/src/main.rs`，254 行；`invoke_handler!` 里 27 个命令，**新命令一律追加在末尾**，见「禁止」）。
- 启动期配置与安全：`config.rs`（一个 schema、一条解析路径）、`paths.rs`（配置文件定位）、`bootstrap.rs`（引导顺序 + CSP 注入）、`token.rs`（每次启动的本机运行 token）、`state.rs`（不进 IPC 的原生状态）。
- 本地安全桥：受控权限下向 Vue 页面暴露本地系统能力——文件/目录选择、安全存储、系统信息（`src-tauri/src/{commands,dto,preflight,filesystem,secure_store,system}`）。
- **本机运行目录与用户设置**（CHG-058）：`app_paths.rs` 解析 Desktop 自己的四个根（数据根 / `versions` / 日志根 / 缓存根；装机态与开发态两套布局，**缓存根不在数据根之内**），读方只用纯函数、写方才 `prepare`；`settings.rs` 读写数据根下的 `settings.toml`（`schema_version` + 同级临时文件 + `rename` 原子替换；损坏时保留原文件并报错，不静默清空）。`save_dir` **目前没有消费方**——页面能改它，不代表下载会按它落盘。
- **本机存储、日志与清理命令面**（CHG-058）：`storage.rs`（可用空间与目录占用）、`cleanup.rs`（只删可安全再生的文件与已轮转归档）、`diagnostic.rs`（脱敏诊断包）、`logging/reader.rs`（列日志文件与读尾部，**列表即白名单**）、`commands/{storage,cleanup,diagnostic,settings,reveal}.rs`。它们只服务 Desktop 自己的「本机设置」页，不是第二套业务后端。
- Agent 生命周期：Local Agent Sidecar 的启动、停止、健康检查、进程恢复，以及 Desktop 与 Local Agent 的本地通信（`src-tauri/src/sidecar/`、`http/`、`src-tauri/binaries`）。**通信是普通 HTTP**：Agent 侧有 `text/event-stream` 端点（`local_api/server.py`），但 Desktop 不消费流，`local_agent_task_status` 取的是状态快照。
- 客户端交付：原生依赖、Sidecar 集成、安装包、版本检测、签名、更新、跨平台打包（`src-tauri/src/updater`、`scripts/`）。支持 Windows x64、macOS Intel、macOS Apple Silicon。
- **Desktop 自己的日志**（`src-tauri/src/logging/`）：目录解析、装配、轮转与保留、target 白名单、脱敏，以及**读取面**（`reader.rs`）。与 Agent 的日志**各自一套、互不转存**——Agent 的业务日志归 `../wt-media-agent` 的 `agent.log`，Desktop 只记自己的生命周期（启动退出、配置加载、Agent 启停与健康检查、sidecar 异常退出）。**没有容量限额**：活文件恒为 `desktop.log`，归档是 `desktop.log.<YYYY-MM-DD-HH>`，超过保留天数（默认 14）的归档由 `file-rotate` 按天删除。

## 本仓库不拥有

- **业务 Vue 页面源码** → `../wt-media-cloud/web`（`web/src/apps/desktop` 与 `web/src/modules`）；本仓库的 `../.generated/frontend` 只是构建产物
- 业务规则、业务数据模型、正式任务管理、Cloud MySQL → `../wt-media-cloud`
- BitBrowser、Playwright、FFmpeg 的实际执行 → `../wt-media-agent`（Agent 内部如何执行不属于 Desktop）

## 需求路由

| 需求是 | 去哪里 |
|---|---|
| 改 Agent Sidecar 启停/健康检查 | `src-tauri/src/sidecar/`（两条 spawn 路径都在此，且共用同一组四个环境变量）。启停与健康检查的生命周期记录（`agent.supervisor`）在 `src-tauri/src/commands/agent.rs` |
| 改 Desktop 日志（目录、级别、轮转、保留、脱敏、新增 target） | `src-tauri/src/logging/`；新增 target 要同时改 `targets.rs` 的 `OWNED_TARGETS`（枚举断言钉着） |
| 改本机运行目录解析（数据根 / `versions` / 日志根 / 缓存根） | `src-tauri/src/app_paths.rs`。**不动 `paths.rs`**——那是配置文件定位器；日志目录仍委托 `logging/paths.rs` |
| 改用户设置（`settings.toml` 的键、schema 版本、原子替换） | `src-tauri/src/settings.rs`；线上形状在 `dto/settings.rs`（**字段名是契约**） |
| 改存储与日志的读取（占用、列文件、读尾部、级别筛选） | `src-tauri/src/storage.rs`、`logging/reader.rs`，命令在 `commands/storage.rs`。**列表即白名单**：先列目录再按名查找 |
| 改清理规则（删什么、留什么、释放字节怎么算） | `src-tauri/src/cleanup.rs`（规则本体）+ `commands/cleanup.rs`（接线）；保护面按**路径**判、按**种类**例外 |
| 改脱敏诊断包（形状、条目、上限、摘要值） | `src-tauri/src/diagnostic.rs`、`commands/diagnostic.rs`、`dto/diagnostic.rs`；**脱敏在门口**——新增进包的字符串都要过 `mask` |
| 改「本机设置」页或日志查看器 | `../wt-media-cloud/web`（`web/src/apps/desktop/features/local-settings`、`local-logs`；**不在本仓库**） |
| 改本地通信（回环客户端、运行 token） | `src-tauri/src/http/local_agent.rs` |
| 改 Cloud 出站调用 | `src-tauri/src/http/cloud.rs` |
| 改配置键、发布默认值、配置文件定位 | `src-tauri/src/config.rs`、`paths.rs`、`resources/desktop.production.toml` |
| 改 CSP | `src-tauri/resources/desktop.production.toml` 的 `browser.csp_connect_src` + `src-tauri/src/bootstrap.rs`（**不在 `tauri.conf.json`**） |
| 改页面能看到的非敏感配置 | `src-tauri/src/commands/public_config.rs`（只返回 `cloud_base_url`/`local_agent_port`/`environment`） |
| 改 Tauri 安全桥（文件、存储、系统信息） | `src-tauri/src/{commands,filesystem,secure_store,system}/`（后三个目前是 3 行空壳） |
| 改命令载荷形状 | `src-tauri/src/dto/`（**字段名与 serde 属性是契约**） |
| 改敏感流程的 Cloud 预检 | `src-tauri/src/preflight.rs` |
| 改 Windows/macOS 安装包、签名、更新 | `src-tauri/src/updater/`、`scripts/build-release-macos.sh` 等 |
| 改 Desktop 使用的 Vue 业务页面 | `../wt-media-cloud/web`（**不在本仓库**） |
| 改窗口、托盘、退出逻辑 | `src-tauri/src/main.rs` |
| 改页面权限边界 | `src-tauri/capabilities/default.json` |

## 禁止

- 复制 Cloud 的业务规则、业务数据模型和正式任务管理能力。
- 直接连 Cloud MySQL。
- 直接承担 FFmpeg、BitBrowser 或 Playwright 业务执行（归 Local Agent）。
- 把 Tauri Rust 层变成第二套业务后端。
- 重复开发已有的 Vue 页面、Store、业务组件和 API 调用层。
- 绕过 Cloud 授权或 Local Agent 执行校验直接操作本地敏感资源。
- Vue 直接访问 Local Agent 动态端口或 Token（必须走 Rust 代理）。
- 在 `logging/setup.rs` 之外装配日志（`set_global_default` 每进程只能一次，第二个装配点不会报错、只会静默失效），或把 Agent 的 stdout 全量转存进 `desktop.log`。
- 在 `invoke_handler!` **中间**插入新命令：既有命令的参数对象被前端按**精确相等**断言（`localAgentService.test.js`），新命令一律**追加在末尾**。
- 给读取或清理命令加**路径参数**：读取面靠「列表即白名单」（先列目录再按名查找），清理靠「根 + 种类」判定；收了路径就等于把保护面交给调用方。

## 上下文加载顺序

1. 本文件（职责与路由）
2. `AGENTS.md` 与 `CLAUDE.md`（边界与规则）
3. `DIRECTORY_MAP.md`（目录导航；含 Vue 产物来源与集成方式）
4. 只读目标模块的代码、直接依赖与 `tests/`

治理上下文（当前 CHG、契约）在 `../wt-media-workspace`，按其 `.ai/CURRENT_CONTEXT.md` 指引加载。禁止默认扫描 `target/`、`../.generated/`、`src-tauri/gen/`、`../generated`。
