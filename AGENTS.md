# WT Media Desktop（Codex 入口）

## 项目定位

本仓是 WT Media 的 Tauri 2 原生客户端工程，负责 Windows/macOS 桌面运行环境、受控系统桥、Local Agent Sidecar 生命周期与安装交付。
业务 Vue 页面源码由 Cloud 的 `web/` 维护，Desktop 集成构建产物；正式业务状态归 Cloud，运营电脑上的实际执行归 Agent。

## 关联工程

- `wt-media-cloud`：正式业务事实、API、Desktop Vue 业务源码及已确认的云端执行能力。
- `wt-media-agent`：浏览器、登录环境、本地文件与平台操作等受控执行。
- `wt-media-workspace`：系统级产品、架构、跨仓协议、决策和 Delivery；关联工程物理路径由其 `config/repository-map.yaml` 维护。

## 任务执行

用本仓 [AGENT-INDEX.md](AGENT-INDEX.md) 确认职责和工作规则；目标位置不明确时用 [DIRECTORY_MAP.md](DIRECTORY_MAP.md) 定位原生代码。
任务关联 Workspace CHG 时，读取其 Desktop 范围和 References；单仓分析、定位和明确的小修改可直接处理。按任务读取相关代码、契约与测试，不默认全仓扫描。

## 相关文档

- 本仓 `contracts.lock.json`：消费的契约版本；`tests/`：原生与发布相关验证。
- Workspace 的 `docs/product/`、`docs/engineering/`、`docs/contracts/`、`docs/decisions/`：系统级事实来源；`delivery/`：CHG 与交付状态。
- Workspace 发起或关联 CHG 的任务，可从 `.ai/CURRENT_CONTEXT.md` 获取执行快照，并以对应 CHG 确认范围。

## 相关约束

- 不复制 Cloud 业务规则、数据模型或 Vue 业务页面，也不直接连接 Cloud MySQL。
- Vue 不直接获取 Local Agent 动态端口或运行 token；本地能力经受控 Rust 桥调用。
- 浏览器与本地文件执行归 Agent，云端视频合成归 Cloud；Workspace 不成为 Desktop 运行时依赖。
