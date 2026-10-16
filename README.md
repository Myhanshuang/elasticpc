# ElasticPC

保留现有 Windows / Manjaro 原生双启动，同时逐步建立对等 Guest 管理和独立服务域。本仓库当前实现 **只读发现、规划、SQLite 持久恢复、真实 QEMU COW 与无盘 Linux 启动实验，以及 libvirt 测试驱动适配器**。现有 Windows/Manjaro 安装尚未导入，也不是可安装的 Hypervisor。

原始需求：[开发执行规格 v0.2](ElasticPC_Development_Spec_v0.2.md)。设计复核：[架构决定](docs/architecture.md)。第一轮：[实施与验证报告](docs/implementation-report.md)。持久化阶段：[持久恢复与文件 COW](docs/p1-storage-and-recovery.md)。最新增量：[Linux 启动与 libvirt](docs/p1-guest-and-libvirt.md)。下一步：[里程碑与门槛](docs/milestones.md)。

## 本地体验入口

新增文件镜像控制台体验版，使用 `./elasticpc init`、`start`、`console`、`status`、`stop` 和 `recover`。它通过现有 Rust/SQLite 控制器管理独立 Debian Guest，数据保存在 `.elasticpc/`。操作与适用范围见 [第一个可体验版本](docs/first-preview.md)；它尚不是 Windows/Manjaro 图形双桌面。

## 构建与验收

需要 Linux、Rust/Cargo。依赖由 `Cargo.lock` 固定，默认构建不链接 libvirt，也不要求 root；可选 `native-libvirt` feature 需要 libvirt 开发文件与运行库。

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

缓存已备好时以上命令均可添加 `--offline`。本轮受限环境使用 `CARGO_HOME=/tmp/elasticpc-cargo-home`：从现有只读缓存获取依赖、在 `/tmp` 解包，不修改主机的 Rust 工具链。普通环境不需要这个设置。

## 只读使用

```bash
cargo run -p elasticctl -- doctor --json --redact
cargo run -p elasticctl -- probe --hardware --json --redact
cargo run -p elasticctl -- probe --installations --json --redact
cargo run -p elasticctl -- installations list --json
cargo run -p elasticctl -- import plan --installation '<list 返回的 id>' --mode trial --json
cargo run -p elasticctl -- layout plan --host-size 20GiB --json
```

`probe` 的 `--hardware` 与 `--installations` 必选其一。`--fixture`、`--json`、`--redact`、`--dry-run` 是全局选项。本节发现/诊断/计划命令只读；`--dry-run` 产生相同计划，不调用写入接口。另有下文明确调用的 `lab` 命令，它们只写自己新建的临时文件。`import plan --mode inspect|trial|native` 分别生成只读检查、覆盖层试运行、原盘排他模式的规划。

`plan` 返回完整方案和阻塞条件。`requires_approval=true` 指未来执行需要批准，不表示已经执行或请求当前批准；成功返回 JSON 不表示方案已具备执行条件。`doctor` 以 `known-supported` / `missing-prerequisite` / `needs-verification` 区分窄能力判断，始终声明本版本不能进行物理执行。参数/读取错误返回非零状态，`--json` 下返回结构化错误；可成功生成的退化报告与被阻塞的计划返回 0。

未挂载的卷不自动挂载、解锁或修复。无法读出系统证据时返回 `family: unknown`，不会按 NTFS 顺序猜 Windows 卷。`/dev/sda` 只作为观察路径；计划的设备引用使用稳定身份。缺 `lsblk` / `lspci` / `efibootmgr`、读取超时或权限不足会明确退化。固定外部命令通过 `/usr/bin/timeout` 限时 5 秒，环境强制 `LC_ALL=C`；不提供任意 shell 执行接口。

脱敏结果带 `redacted=true`，只能用于展示。需要生成计划时应使用本地未脱敏 ID，不能把 `installation-001` 等占位符当作设备身份。项目没有遥测。

## 持久恢复与 COW 实验

```bash
cargo run -p elasticctl -- readiness --json --redact
cargo run -p elasticctl -- lab recovery --json
cargo run -p elasticctl -- lab file-cow --json
cargo run -p elasticctl -- lab file-cow --tool-dir /absolute/verified/qemu/bin --json
cargo run -p elasticctl -- --dry-run lab recovery --json
```

`readiness` 检查 KVM/工具可见性、内存、稳定磁盘身份、resume 对应关系和服务数据挂载元数据；不连接可能触发服务启动的 libvirt socket。显式 `--check-kvm-api` 可只读查询 KVM API 版本；默认扫描不打开 KVM 设备，dry-run 也不会执行 ioctl。

`lab recovery` 使用真实 SQLite 和模拟 Guest；`lab file-cow` 使用真实 QEMU 工具及新建的 8 MiB 文件底盘。两个实验完成后清理临时文件，不接受真实设备或用户已有镜像路径。`--dry-run` 不创建任何实验文件或执行工具。缺工具时返回 `skipped` / 退出码 3，失败或拒绝为 1，通过为 0。`--tool-dir` 仅用于调用方已验证来源的固定名称 QEMU 工具，不提供任意命令入口。

本轮已在临时解包并验签的 QEMU 11.1.2 上完成实测，[报告](reports/p1-file-cow-lab.json)记录 14 项检查与基底散列。没有安装系统软件包或改变服务。详细范围及复现命令见[本轮文档](docs/p1-storage-and-recovery.md)。

## Linux 启动与 libvirt 实验

```bash
./target/debug/elasticctl lab guest-boot --kernel /absolute/path/to/vmlinuz \
  --tool-dir /absolute/verified/bin --firmware-dir /absolute/verified/firmware --json
cargo build -p elasticctl --locked --features native-libvirt
./target/debug/elasticctl lab libvirt-test --json
```

`guest-boot` 默认使用 TCG；显式 `--kvm` 启动须有主机测试授权，且失败不回退 TCG。两种模式均固定 1 vCPU、256 MiB 内存和项目构建的最小 PID 1。不接入磁盘或网络，以串口标记和正常关机作为成功证据。`libvirt-test` 只连接进程内的 `test:///default` 模拟驱动，验证真实绑定 API；未启用 native feature 时返回未运行。两者均支持无副作用的 `--dry-run`，不能指定生产 libvirt URI。[设计与复现说明](docs/p1-guest-and-libvirt.md)说明证据范围和临时依赖环境。

## 完全离线的示例

```bash
cargo run -p elasticctl -- --fixture fixtures/acacia-redacted doctor --json
cargo run -p elasticctl -- --fixture fixtures/acacia-redacted probe --hardware --json
cargo run -p elasticctl -- --fixture fixtures/acacia-redacted import plan \
  --installation linux-11111111-1111-1111-1111-111111111111 --mode trial --json
cargo run -p elasticctl -- --fixture fixtures/acacia-redacted layout plan --host-size 20GiB --json
cargo run -p elasticctl -- --fixture fixtures/generic/windows-on-fourth-partition probe --installations --json
```

`fixtures/acacia-redacted` 是依据文档重建的匿名输入，**不是本次实机采集**。实际尚未确定的 DRM 归属保留未知；另有合成 fixture 验证映射解析。[来源说明](docs/hardware-fixture-notes.md)列出替换与构造字段。

## 模块与安全边界

- `elastic-core`：强类型安装/域/设备/计划，schema 与脱敏。
- `elastic-probe`：有限读取、fixture/实机共用解析、PCI/IOMMU/DRM/磁盘/EFI/已挂载安装证据。
- `elastic-planner`：导入/布局、内存预算、`DomainSpec`、覆盖层指纹、层级租约、`MockHypervisor` / `MockController`。
- `elastic-state`：SQLite 版本化检查点、事务 CAS 和安全文件路径。
- `elastic-controller`：SQLite 日志、可替换生命周期后端、进程互斥及 mock/libvirt TEST 恢复实验。
- `elastic-lab`：普通文件 raw/qcow2 隔离与过期基底检查、无盘 Linux 启动（默认 TCG，显式授权后可选 KVM）。
- `elastic-virt-libvirt`：保守 XML 资源观察与可选的真实 libvirt 测试驱动调用。
- `elasticctl`：发现/规划/readiness，以及明确调用的临时文件实验。

Mock 可独立使用内存日志；`DurableMockController` 将日志同步保存到 SQLite，并用进程锁隔离生命周期操作。元数据真实持久化，Guest 后端仍然模拟。恢复依据新观测，未知 I/O 状态保留租约；停止调用报错不等于设备空闲。`DomainSpec` 明确 `executable=false`，为后续 libvirt adapter 提供计划输入，没有假造可执行 XML。

[测试索引](tests/README.md)列出 golden 与不变量验收。普通文件 raw/COW 的底盘不变性和无盘 Linux 的 TCG 与 KVM 启动均已实测，见 [KVM 验收](docs/p1-kvm-verification.md)；真实块设备、现有安装导入、原生引导恢复、Windows/TPM/BitLocker、VFIO 复位和显示切换仍待验证。libvirt TEST 已接入同一持久控制器，生产执行路径仍未开放。启用 native feature 后可运行 `elasticctl lab libvirt-recovery --json`，详见[控制器集成与主机验证计划](docs/p1-controller-host-validation.md)。

本版本没有真实 `import apply`、`vm start/stop`、`boot native` 或 `display switch`，CLI 没有修改真实分区、ESP/NVRAM、驱动绑定或生产服务的代码路径。P2 镜像脚本仅在显式启动的隔离 Guest 内管理其文件磁盘与 OVMF 变量。

## 私有实例与真实恢复验收

已增加私有文件磁盘 Guest 与真实控制器进程恢复实验，复用现有 SQLite 控制器。`make check` 为离线软件检查；`make private-lab-dry-run` 只显示计划；已获得主机实验授权后可用 `make private-lab` 运行独立临时 libvirt/KVM 实例。详细边界、报告及依赖准备见 [私有 Guest 恢复验收](docs/p1-private-guest-recovery.md)。这不开放生产连接或真实磁盘执行。

## P2 最小 Host 镜像

已完成 Debian Host 文件镜像构建、独立 GRUB 恢复入口、Guest 内 EFI 安装/卸载及再次启动的六阶段隔离验收，并提供两套物理布局规划。交付镜像位于 `target/p2-host-release/`；真实部署门禁尚未通过。`make p2-dry-run` 查看无副作用计划；显式授权的隔离 KVM 构建入口为 `make p2-host`。依赖准备、证据范围和复现步骤见 [P2 构建与验收](docs/p2-build-and-verification.md)，真实 SSD/EFI 门禁见 [P2 安装方案](docs/p2-host-installation-plan.md)。这些脚本不是现有主机安装器，不能用隔离 Debian 的回退代替 Windows/Manjaro 原生启动验收。

## P3 文件 Trial 增量

新增 `scripts/trial_image.py`，准备并验证普通文件镜像 Trial：独立底盘副本、COW 覆盖层、固件变量、完整指纹和进程互斥。`make p3-check` 运行文件测试，`make p3-dry-run` 显示隔离验证计划。真实 KVM 正常启动、SIGKILL 后再次启动及变更底盘拒绝均已通过，详见 [P3 文件 Trial](docs/p3-file-trial.md)。该增量尚未接入生产控制器，也未导入真实 Windows/Manjaro。
# 双系统导入与设备交接开发入口

新增 `scripts/boot_bundle.py` 启动文件检查/暂存、`trial_image.py invalidate-for-native` 文件 Trial 失效保护，以及 `elasticctl import native-return` / `gpu-plan` 非执行规划。使用范围、保留驱动策略和实机阻塞见 [双系统交接说明](docs/dual-system-handoff.md)。这些增量不表示真实 Windows/Manjaro 或 GPU 直通已验收。
