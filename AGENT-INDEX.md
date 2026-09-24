# wt-media-desktop Agent Index

## 依赖

关于文档等事实都在`../wt-media-workspace`，需要执行时优先考虑对应的约束边界

## 定位

本仓库是 WT Media 的 **Tauri 2 原生客户端工程**：Windows 与 macOS 客户端的原生能力、运行环境、进程生命周期与打包交付。

Desktop 是**客户端控制壳，不是第二套业务系统**。

## 本仓库拥有

- 客户端生命周期：Tauri 初始化、窗口、应用退出、系统托盘、运行状态（`src-tauri/src/main.rs`，约 115 行）。
- 启动期配置与安全：`config.rs`（一个 schema、一条解析路径）、`paths.rs`（配置文件定位）、`bootstrap.rs`（引导顺序 + CSP 注入）、`token.rs`（每次启动的本机运行 token）、`state.rs`（不进 IPC 的原生状态）。
- 本地安全桥：受控权限下向 Vue 页面暴露本地系统能力——文件/目录选择、安全存储、系统信息（`src-tauri/src/{commands,dto,preflight,filesystem,secure_store,system}`）。
- Agent 生命周期：Local Agent Sidecar 的启动、停止、健康检查、进程恢复，以及 Desktop 与 Local Agent 的本地通信（`src-tauri/src/sidecar/`、`http/`、`src-tauri/binaries`）。**通信是普通 HTTP**：Agent 侧有 `text/event-stream` 端点（`local_api/server.py`），但 Desktop 不消费流，`local_agent_task_status` 取的是状态快照。
- 客户端交付：原生依赖、Sidecar 集成、安装包、版本检测、签名、更新、跨平台打包（`src-tauri/src/updater`、`scripts/`）。支持 Windows x64、macOS Intel、macOS Apple Silicon。
- **Desktop 自己的日志**（`src-tauri/src/logging/`）：目录解析、装配、轮转与限额、target 白名单、脱敏。与 Agent 的日志**各自一套、互不转存**——Agent 的业务日志归 `../wt-media-agent` 的 `agent.log`，Desktop 只记自己的生命周期（启动退出、配置加载、Agent 启停与健康检查、sidecar 异常退出）。

## 本仓库不拥有

- **业务 Vue 页面源码** → `../wt-media-cloud/web`（`web/src/apps/desktop` 与 `web/src/modules`）；本仓库的 `../.generated/frontend` 只是构建产物
- 业务规则、业务数据模型、正式任务管理、Cloud MySQL → `../wt-media-cloud`
- BitBrowser、Playwright、FFmpeg 的实际执行 → `../wt-media-agent`（Agent 内部如何执行不属于 Desktop）

## 需求路由

| 需求是 | 去哪里 |
|---|---|
| 改 Agent Sidecar 启停/健康检查 | `src-tauri/src/sidecar/`（两条 spawn 路径都在此，且共用同一组四个环境变量）。启停与健康检查的生命周期记录（`agent.supervisor`）在 `src-tauri/src/commands/agent.rs` |
| 改 Desktop 日志（目录、级别、轮转、保留、脱敏、新增 target） | `src-tauri/src/logging/`；新增 target 要同时改 `targets.rs` 的 `OWNED_TARGETS`（枚举断言钉着） |
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

## 上下文加载顺序

1. 本文件（职责与路由）
2. `AGENTS.md` 与 `CLAUDE.md`（边界与规则）
3. `DIRECTORY_MAP.md`（目录导航；含 Vue 产物来源与集成方式）
4. 只读目标模块的代码、直接依赖与 `tests/`

治理上下文（当前 CHG、契约）在 `../wt-media-workspace`，按其 `.ai/CURRENT_CONTEXT.md` 指引加载。禁止默认扫描 `target/`、`../.generated/`、`src-tauri/gen/`、`../generated`。
