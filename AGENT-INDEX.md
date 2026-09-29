# WT Media Desktop Agent Index

本文件只记录 Desktop 的长期工作规则，不记录当前 Milestone、CHG、临时任务状态或本机路径。

## 1. 仓库职责

- Desktop 是 Tauri 2 原生客户端，负责窗口与应用生命周期、受控系统桥、Local Agent Sidecar 和安装交付。
- Cloud 裁决正式业务状态，维护 HTTP API、Desktop Vue 业务源码和已确认的云端视频合成能力。
- Agent 负责运营电脑上的浏览器、登录环境、本地文件落地和其他实际执行。
- Desktop 通过 Rust 桥提供本机能力，不复制 Cloud 业务后端或 Agent 执行逻辑。
- Workspace 维护系统级 Product、Architecture、Contract、Decision 和 Delivery；Desktop 运行时及安装产物不依赖 Workspace。

## 2. Workspace 协作

- 分析、定位和明确的小范围单仓修改可以直接进行，不自动创建 CHG。
- 已关联 CHG 时，以其目标、范围和 References 为任务依据，只实施已确认的 Desktop 范围。
- References 是优先入口，不限制任务确实需要的进一步查证。
- 涉及跨仓 Contract、系统架构、仓库职责或扩大 CHG 范围时，先回 Workspace 更新共同定义。
- Workspace 发起的任务直接使用当前 Workspace 上下文；独立执行需要 Workspace 时使用已提供的根目录或环境配置，无法定位则要求提供路径。
- 不复制 Workspace 文档，也不在 Desktop 建立第二套 Delivery、任务状态或临时 context。

## 3. 上下文加载

按任务逐步读取：

1. 理解当前任务；有关联 CHG 时读取对应 CHG。
2. 按需要读取相关 Product、Engineering、Contract 或 Decision。
3. 目标位置不明确时用 `DIRECTORY_MAP.md` 定位原生区域和验证入口。
4. 进入目标模块后，再读取直接依赖、调用方和相关测试。

默认不做全仓代码扫描，不读取 `delivery/completed`、历史材料和全量 Skills，也不扫描构建、缓存、临时或生成目录。任务需要时可以扩大范围，并说明目的。

## 4. 本仓架构

- Vue 业务源码由 Cloud 的 `web/` 维护；Desktop 消费其构建产物，不建立第二套业务前端。
- Vue 通过受控 Tauri 命令访问本机能力；Local Agent 动态端口和运行 token 由 Rust 层持有，不直接交给页面。
- 敏感 Token 使用操作系统安全存储；原生敏感状态不作为普通页面数据暴露。
- 命令载荷、serde 字段和权限声明构成前端接口，变化时核对 Cloud 的 Vue 调用与契约锁。
- Sidecar 管理 Local Agent 生命周期和本地通信；实际浏览器、文件落地和平台操作由 Agent 执行。
- 本机文件读取、打开和清理保持受控范围，不接受页面任意路径；下载字节由 Local Agent 落盘。
- Desktop 只记录自身日志，不全量转存 Agent 输出。

## 5. 修改与验证

- 从目标代码开始调查；位置不明确时使用 `DIRECTORY_MAP.md`。
- 实现事实以当前代码、契约锁和 Cloud Vue 消费点为准。
- 验证从最小相关范围开始，优先使用仓库已有脚本和测试方式。
- 命令、权限或本地桥变化时核对前端调用；Sidecar、打包或更新变化时检查受影响的平台与产物接线。
- 不默认执行全平台构建，不修改生成产物，也不覆盖、还原或提交他人已有工作区改动。
- CHG 任务完成后按 Workspace 当前 Delivery 规则回写状态。
