# WT Media Desktop


## 必须遵守

Follow `AGENT-INDEX.md` for repository boundaries.


## Responsibility

Desktop 拥有 Tauri 壳、Rust 系统桥、Local Agent 生命周期、安全存储、文件选择、更新器与跨平台打包。

Desktop 是客户端控制壳，不是第二套业务系统。

## Structure

- `src-tauri/src/main.rs`: 只剩启动序列（`mod` 声明、配置引导、CSP 注入、Builder 装配、单实例守卫、`generate_handler!` 的 27 个命令）。
- `src-tauri/src/config.rs`、`paths.rs`、`bootstrap.rs`、`token.rs`、`state.rs`: 启动期配置与安全（schema 与解析路径、配置文件定位、引导顺序与 CSP 注入、每次启动的运行 token、不进 IPC 的原生状态）。
- `src-tauri/src/commands/`、`dto/`、`preflight.rs`: 暴露给 Vue 的命令、按消费方分组的载荷结构体（字段名即契约）、敏感流程共用的 Cloud 预检。`webview.rs` 原名 `logging.rs`（纯重命名，命令名 `log_js_error` 不变），它报的是 webview 的 JS 错误，不是日志子系统的入口。
- `src-tauri/src/http/`: 两个分开的客户端——`local_agent.rs`（回环、带运行 token）与 `cloud.rs`（地址与凭据逐请求给）。
- `src-tauri/src/sidecar/`: sidecar 启停（两条 spawn 路径共用同一组四个环境变量）与输出保留（内存环形缓冲 200 行）。Agent 的 stdout **不落盘**：存活期间的普通输出不产生记录，退出时恰一条 `agent.supervisor` 记录带末 20 行尾。
- `src-tauri/src/logging/`: Desktop 自己的日志——装配（`setup.rs` 是唯一入口）、目录解析、轮转与限额、target 白名单、脱敏。Dev 也落盘（`<repo>/.local/logs`，装态 `~/Library/Logs/WTMedia/Desktop`）。
- `src-tauri/src/local_agent/`: 只余纯契约类型 `BoundNodeFacts`。
- `src-tauri/src/{filesystem,secure_store,system,updater}/`: 目前是 3 行占位空壳，尚无实现。
- `src-tauri/resources/desktop.production.toml`: 编译进二进制的生产配置（CSP 也在这里，不在 `tauri.conf.json`）。
- `src-tauri/binaries`: Sidecar 二进制组件。
- `src-tauri/capabilities`: Tauri 权限能力。
- `contracts.lock.json`: 消费的契约版本锁。
- `scripts`: 构建、发布、签名、健康检查脚本（含 `packaging` 相关发布资产流程）。
- 业务 Vue 页面源码在 `../wt-media-cloud/web`：开发时 `beforeDevCommand` 指向该工程，发布时由 `../wt-media-workspace/scripts/build-desktop.sh` 产出前端到 `../.generated/frontend`（`frontendDist`）。本仓库不维护第二套 Vue 源码，也**没有 `src/` 顶层目录**。

## Rules

- Desktop 不复制 Cloud 业务 Web。
- Vue 不直接访问 Local Agent 动态端口或 Token。
- Rust 代理 Local Agent 的 HTTP。Agent 侧虽有 `text/event-stream` 端点，Desktop 不消费流（`local_agent_task_status` 是状态快照），不要在文档或代码注释里把这条写成已实现。
- 敏感 Token 走 OS 安全存储，不进 localStorage。
- 平台特定行为放 Rust 系统桥模块，不散落在业务 UI 代码。
- `src-tauri/gen/`（Tauri 生成内容）禁止手改。
