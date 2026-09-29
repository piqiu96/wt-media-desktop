# WT Media Desktop（Claude Code 入口）

## 项目定位

本仓是 WT Media 的 Tauri 2 原生客户端工程，负责 Windows/macOS 桌面运行环境、受控系统桥、Local Agent Sidecar 生命周期与安装交付。Desktop 消费 Cloud 维护的 Vue 业务页面构建产物。

## 关联工程

- `wt-media-cloud`：正式业务事实、API、Desktop Vue 业务源码和已确认的云端执行能力。
- `wt-media-agent`：浏览器、登录环境、本地文件与平台操作等受控执行。
- `wt-media-workspace`：维护系统级产品、架构、跨仓协议、决策和 Delivery；关联工程物理路径由 Workspace 的 `config/repository-map.yaml` 维护。

## 任务执行

先用 [AGENT-INDEX.md](AGENT-INDEX.md) 确认本仓规则；目标位置不明确时使用 [DIRECTORY_MAP.md](DIRECTORY_MAP.md) 定位原生代码。
任务关联 Workspace CHG 时，读取其 Desktop 范围和 References；单仓分析、定位和明确的小修改可直接处理。
按任务逐步读取相关代码、文档和测试，不默认全仓扫描。

## 相关文档

- 本仓 `contracts.lock.json`：消费的契约版本；`tests/`：原生与发布相关验证。
- Workspace `docs/product/`、`docs/engineering/`、`docs/contracts/`、`docs/decisions/`：系统级事实来源。
- Workspace `delivery/`：Milestone、CHG 和交付状态；关联任务可从 `.ai/CURRENT_CONTEXT.md` 获取执行快照。

## 相关约束

- Vue 业务源码、正式业务状态和云端视频合成归 Cloud；Desktop 不建立第二套业务前端。
- 浏览器与本地文件实际操作归 Agent；Vue 的本机能力经受控 Rust 桥调用。
- Workspace 不成为 Desktop Runtime 或安装产物的依赖。
