# WT Media Desktop


## 必须遵守

Follow `AGENT-INDEX.md` for repository boundaries.


## Responsibility

Desktop 拥有 Tauri 壳、Rust 系统桥、Local Agent 生命周期、安全存储、文件选择、更新器与跨平台打包。

Desktop 是客户端控制壳，不是第二套业务系统。

## Structure

- `src-tauri/src`: Rust 桥模块（`main.rs` 入口、`commands`、`filesystem`、`local_agent`、`secure_store`、`system`、`updater`）。
- `src-tauri/binaries`: Sidecar 二进制组件。
- `src-tauri/capabilities`: Tauri 权限能力。
- `contracts.lock.json`: 消费的契约版本锁。
- `scripts`: 构建、发布、签名、健康检查脚本（含 `packaging` 相关发布资产流程）。
- 业务 Vue 页面源码在 `../wt-media-cloud/web`：开发时 `beforeDevCommand` 指向该工程，发布时由 `../wt-media-workspace/scripts/build-desktop.sh` 产出前端到 `../.generated/frontend`（`frontendDist`）。本仓库不维护第二套 Vue 源码。

## Rules

- Desktop 不复制 Cloud 业务 Web。
- Vue 不直接访问 Local Agent 动态端口或 Token。
- Rust 代理 Local Agent HTTP 与 SSE。
- 敏感 Token 走 OS 安全存储，不进 localStorage。
- 平台特定行为放 Rust 系统桥模块，不散落在业务 UI 代码。
- `src-tauri/gen/`（Tauri 生成内容）禁止手改。
