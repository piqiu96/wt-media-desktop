# WT Media Desktop：目录地图

本文件只用于代码定位、验证入口和默认扫描边界。架构与执行规则见 `AGENT-INDEX.md`，实现事实以当前代码为准。

## Tauri 原生

| 位置 | 主要内容 | 适用任务 |
| --- | --- | --- |
| `src-tauri/src/main.rs`、`src-tauri/src/bootstrap.rs` | 进程入口、启动装配和生命周期 | 窗口、启动、退出 |
| `src-tauri/src/config.rs`、`src-tauri/src/paths.rs`、`src-tauri/src/state.rs`、`src-tauri/src/token.rs` | 配置定位、原生状态和运行 token | 配置、本机鉴权 |
| `src-tauri/src/commands/`、`src-tauri/src/dto/` | 受控命令与 Vue 消费的载荷 | IPC 接口、参数、返回值 |
| `src-tauri/src/http/`、`src-tauri/src/preflight.rs` | Cloud 和 Local Agent 出站客户端、敏感流程预检 | 本地通信、Cloud 调用 |
| `src-tauri/src/sidecar/`、`src-tauri/binaries/` | Agent Sidecar 生命周期和打包二进制 | 启停、健康检查、集成 |
| `src-tauri/src/app_paths.rs`、`src-tauri/src/settings.rs` | 运行目录和用户设置 | 本机设置、保存位置 |
| `src-tauri/src/storage.rs`、`src-tauri/src/cleanup.rs`、`src-tauri/src/diagnostic.rs` | 存储、清理和诊断 | 本机维护功能 |
| `src-tauri/src/logging/` | Desktop 日志装配和读取 | 日志、诊断与可观测性 |
| `src-tauri/src/filesystem/`、`src-tauri/src/secure_store/`、`src-tauri/src/system/`、`src-tauri/src/updater/` | 原生能力与更新模块；以当前实现为准 | 对应系统能力 |
| `src-tauri/capabilities/`、`src-tauri/resources/`、`src-tauri/tauri.conf.json` | 权限、生产配置和构建接线 | 权限、CSP、发布配置 |
| `contracts.lock.json` | 消费的契约版本 | 跨仓接口变化 |

## Vue 来源

Cloud 仓的 `web/src/apps/desktop/` 与 `web/src/modules/` 维护 Desktop Vue 业务页面和共用模块；本仓消费其构建产物。具体集成路径以 `src-tauri/tauri.conf.json` 为准。

## 测试与验证入口

| 范围 | 优先入口 |
| --- | --- |
| Rust 原生能力 | `src-tauri/Cargo.toml`、相关模块测试和 `tests/` |
| Tauri 命令与权限 | `src-tauri/src/commands/`、`src-tauri/src/dto/`、`src-tauri/capabilities/`、Cloud Vue 调用方 |
| Cloud／Agent 协议 | `contracts.lock.json`、`src-tauri/src/http/` 和对应提供方 |
| Sidecar、打包与更新 | `src-tauri/src/sidecar/`、`src-tauri/src/updater/`、`scripts/` 中已有验证 |

优先复用仓库已有验证方式，不在本文件复制具体命令。

## 工具

- `scripts/`：开发、验证、打包和发布脚本。
- `bin/`：本地进程控制及运行辅助。

## 默认跳过

日常代码检索默认跳过：

- Rust 构建产物，如 `target/`。
- Tauri 自动生成内容，如 `src-tauri/gen/`。
- 前端构建产物、依赖、缓存、日志和临时文件。
- `.git/` 等版本控制元数据。

任务直接涉及这些内容时再进入。
