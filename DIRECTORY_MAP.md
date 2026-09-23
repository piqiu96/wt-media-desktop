# wt-media-desktop 目录地图

本文只记录实际存在的目录。定位代码时从本文件出发，禁止全仓库扫描。

**特别标注：业务 Vue 页面源码在 `../wt-media-cloud/web`，不在本仓库。** 本仓库只拥有 Tauri 原生壳；`../.generated/frontend` 是构建产物，不是源码。

## 一、Tauri 应用入口

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/main.rs` | Tauri 初始化、窗口、应用退出、系统托盘、客户端运行状态 | 改客户端生命周期 |
| `src-tauri/tauri.conf.json` | 应用配置：`frontendDist: ../.generated/frontend`、devUrl 5174、构建命令指向 `../wt-media-cloud/web` | 改窗口/构建配置 |
| `src-tauri/build.rs`、`src-tauri/gen/`、`src-tauri/icons/` | 构建脚本、生成内容、图标 | 改打包资源 |

## 二、Rust 命令与安全桥

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/commands/` | 暴露给 Vue 页面的 Tauri 命令 | 改命令接口 |
| `src-tauri/src/filesystem/` | 文件/目录选择等受控本地文件能力 | 改文件桥 |
| `src-tauri/src/secure_store/` | OS 安全存储（敏感 Token 不进 localStorage） | 改安全存储 |
| `src-tauri/src/system/` | 系统信息与本地系统能力 | 改系统能力桥 |
| `src-tauri/capabilities/default.json` | Tauri 权限能力声明 | 改权限边界 |

规则：Vue 页面不直接访问 Local Agent 动态端口或 Token；平台特定行为放 Rust 桥模块，不散落在业务 UI。

## 三、Agent Sidecar 生命周期

| 路径 | 职责 | 何时进入 |
|---|---|---|
| `src-tauri/src/local_agent/` | Local Agent Sidecar 启动、停止、健康检查、进程恢复；Rust 代理 Local Agent HTTP 与 SSE | 改 Sidecar 启停、本地通信 |
| `src-tauri/binaries/` | Sidecar 二进制组件（当前含 `wt-media-agent-aarch64-apple-darwin`） | 改 Sidecar 集成 |

Agent 内部如何执行浏览器操作或 FFmpeg，不属于本仓库（归 `../wt-media-agent`）。

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
