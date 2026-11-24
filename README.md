# wt-media-desktop

Tauri 2 desktop shell for the modular social media operations platform.

This repository owns the native shell only. The business Vue pages are built in
`../wt-media-cloud/web` and arrive here as `.generated/frontend/`.

## Responsibilities

- Tauri lifecycle: windows, tray, exit, client run state.
- Startup configuration and security: config schema, file location, per-launch
  runtime token, and CSP injection.
- Native bridge under capability control: file selection, secure storage, system
  info, updater.
- Local Agent lifecycle: sidecar start/stop/health/recovery, plus the loopback
  and Cloud HTTP clients.
- Cross-platform packaging for Windows x64, macOS Intel, and macOS Apple Silicon.

## Bootstrap

Requires a Rust/Cargo toolchain. There is no Node package toolchain here: this
repository has no `package.json`, and nothing is installed from `npm`. (A `node`
binary is still used, as a JSON reader, by the release suites under `tests/`.)

```bash
scripts/test.sh          # cargo test --workspace, then the suites in tests/
```

`scripts/test.sh` writes a placeholder at the bundled-sidecar path when nothing
is there yet: Tauri's build script refuses to compile unless that path exists,
and the suite never reads the file's contents. The real sidecar comes from
`scripts/prepare-release-sidecar.sh`, which needs a sibling `wt-media-agent`
checkout. The placeholder and its `exit 1` body are described in the script.

The local development shell is started and stopped through `bin/control.sh`; see
[`scripts/README.md`](scripts/README.md) for the script classification and
`DIRECTORY_MAP.md` for the directory facts.

## Key Directories

- `src-tauri/src`: the Rust shell, split by responsibility — `main.rs` is the
  launch sequence only. See `DIRECTORY_MAP.md` for the full map.
- `src-tauri/resources/desktop.production.toml`: production configuration,
  compiled into the binary as a fallback.
- `src-tauri/binaries`: bundled sidecar component.
- `src-tauri/capabilities`: Tauri permissions.
- `contracts.lock.json`: consumed contract versions.

There is no `src/` directory in this repository.

## Local Agent Control Boundary

The Desktop boundary consumes Cloud-Agent API `v1@2026.07.15.1`, task schemas
`v1@2026.07.15.1`, Local Agent API `v1@2026.07.14.7`, Profile guard events
`profile-guard@2026.07.14.8`, and local status enums `status@2026.07.14.8`.

Vue code does not reach Local Agent ports or tokens directly. It asks
`get_public_config` for the three non-secret facts it is allowed to see
(`cloud_base_url`, `local_agent_port`, `environment`) and calls the sidecar
through the Rust bridge, which attaches the per-launch runtime token.

The `local_agent_bind_session` native command accepts a short-lived one-use Cloud
ticket. The Vue service passes it only as an invoke argument and normalizes the
response so node/permit credentials are retained by the native/Agent boundary
rather than by application state.

## Verification

```text
scripts/test.sh
bin/control.sh status
```

`scripts/test.sh` runs `cargo test --workspace`, Shell regressions under
`tests/op/` and `tests/pack/`, packaging identity regression, and the Windows
subsystem static check. Windows process and real installer tests run separately
in the Workspace Windows release job; see [`tests/README.md`](tests/README.md).
`bin/control.sh status` answers the other question — whether the shell is up and the dev server
is answering — and reports liveness and health as two separate readings.

Cloud 目标环境由 `[cloud].cloud_env = "dev/test/pre/prod"` 显式配置，与应用构建模式分开。
Desktop 启动 Agent 时一起传递 Cloud 地址和 `WT_MEDIA_CLOUD_ENV`；开发应用连接正式 Cloud 时使用 `prod`。

Desktop 身份使用 `environment=dev/test/prod`；Cloud 目标另行使用 `cloud_env=dev/test/pre/prod`。Sidecar 同时收到 `WT_MEDIA_ENV` 和 `WT_MEDIA_CLOUD_ENV`，允许 dev/test 桌面连接 prod Cloud。正式发布保持 release+prod 门禁，测试安装包使用 release+test。

## Desktop 环境身份与本地隔离

Desktop `environment`（dev/test/prod）与 `cloud.cloud_env` 独立。测试桌面连接线上时仍为 Test；开发桌面仍为 Dev。运行时修改资源中的 environment 不会改变已安装应用身份。构建配置通过 `WT_MEDIA_DESKTOP_ENVIRONMENT` 固定原生身份，构建入口同时生成对应环境的 TOML、名称、图标、应用标识和主程序名称。

支持的构建入口：

```sh
# 开发窗口，DEV 图标与独立数据目录
python3 scripts/build-desktop-environment.py --environment dev --profile debug --dev
# 开发安装包，包含随包 Agent，无需启动页面开发服务器
python3 scripts/build-desktop-environment.py --environment dev --profile debug --bundles app,dmg
# 测试安装包；release 编译，TEST 身份，Cloud 目标沿用配置
python3 scripts/build-desktop-environment.py --environment test --bundles app
# 正式安装包；正式流水线和 macOS 发布入口强制 stable 门禁
python3 scripts/build-desktop-environment.py --environment prod --bundles app
```

构建目标按 `--target`、`CARGO_BUILD_TARGET`、Cargo 配置的优先级确定；配置从 `src-tauri/` 向上查找，并读取 Cargo 用户配置。构建、产物定位与 DMG 架构使用同一目标。多目标数组或使用 `include` 的 Cargo 配置需要显式传入一个 `--target`。

Windows 将 `--bundles app` 改为 `--bundles nsis`。图标由既有 `src-tauri/icons/brand-mark.svg` 派生，开发与测试角标的矢量源在 `icons/dev/` 和 `icons/test/`；修改后运行 `node scripts/generate-environment-icons.mjs` 及 `cargo tauri icon <png> --output <目录>`，只保留桌面图标。

macOS 管理目录：

| environment | Application Support 下的数据 | Logs 下的日志 | Caches 下的缓存 |
|---|---|---|---|
| dev | WTMedia/Dev/{Desktop,Agent} | WTMedia/Dev/{Desktop,Agent} | WTMedia/Dev/Desktop |
| test | WTMedia/Test/{Desktop,Agent} | WTMedia/Test/{Desktop,Agent} | WTMedia/Test/Desktop |
| prod | WTMedia/{Desktop,Agent} | WTMedia/{Desktop,Agent} | WTMedia/Desktop |

Windows 使用 `%LOCALAPPDATA%/WTMedia/[Dev|Test/]Desktop/{data,logs,cache}` 和 `Agent/`（其日志为 `Agent/logs`）。默认本地端口为 dev 18765、test 28765、prod 8765。应用标识分别为 `com.wtmedia.desktop.dev`、`.test` 和原有 `com.wtmedia.desktop`，从而隔离单实例与 WebView 登录存储。

Desktop 显式传递 Agent 数据目录及日志文件，沿用 Agent 已有配置能力。自定义 Agent 数据根在非正式环境额外附加 dev/test 子目录；正式路径保持兼容。目录查看、日志、清理和导出复用同一解析结果；测试/开发卸载不得清理正式目录。旧的 `.local` 开发数据不自动迁入；不自动复制正式设置、登录状态或 SQLite。用户手动指定的下载文件夹仍由其选择管理。

正式 tagged 流水线固定 `WT_MEDIA_DESKTOP_RELEASE_CHANNEL=stable`、`WT_MEDIA_DESKTOP_ENVIRONMENT=prod`，编译门禁拒绝 debug、test、dev。测试发布入口固定 test channel，要求 release+test。所有 release 包仍禁止外部 Python 回退。

macOS 环境打包入口在 Tauri 签名后执行现有 Agent 签名修复、更新最终 SHA-256 运行记录并封装外层签名；DMG 只从完成收尾的 app 生成。自定义 Cargo 输出目录与显式目标架构使用 Cargo metadata 和 --target 解析。不可直接把未收尾的 cargo tauri build 产物作为开发／测试交付包。Agent 本机服务使用数字监听地址，不在启动阶段执行反向 DNS。
