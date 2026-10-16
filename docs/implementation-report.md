# P0 / P1 软件部分交付记录

这是第一轮历史验收记录；后续状态见 [P1 持久恢复与 COW 验证](p1-storage-and-recovery.md)。

日期：2026-10-13（Asia/Shanghai）。工作目录：`/home/acacia/elasticpc`。原始规格保持不变。

## 实施内容

创建可编译 Rust workspace 与固定的 `Cargo.lock`：

| 路径 | 已实现 |
|---|---|
| `crates/elastic-core/src/lib.rs`、`redact.rs` | 安装/域/资源/计划的强类型模型、schema、内容计划 ID、脱敏 |
| `crates/elastic-probe/src/` | 有界只读探测、EFI 全量条目、PCI/IOMMU/DRM、磁盘/挂载/swap/resume、安装证据 |
| `crates/elastic-planner/src/import.rs`、`layout.rs` | 三种导入模式与两套条件布局计划，阻塞条件/影响设备/回退/审批信息 |
| `crates/elastic-planner/src/domain.rs`、`resources.rs` | 非执行 DomainSpec、私有 Guest 固件状态、保守 RAM 准入 |
| `crates/elastic-planner/src/leases.rs`、`mock.rs` | 整盘/分区层级租约、完整组所有权、mock 生命周期、6 个崩溃点与观察恢复 |
| `crates/elasticctl/src/` | `doctor`、`probe`、`installations list`、`import plan`、`layout plan` |
| `fixtures/`、`tests/plan-golden/` | 5 组匿名/合成场景、两份完整计划 golden |
| `reports/` | fixture 样例与单独标注的本机脱敏观察 |

设计调整见 [architecture.md](architecture.md)：未知占用直接阻塞；即使 Trial 也不能使用正在被 Host 写入的底盘；重复 stable ID/PARTUUID 隔离；报告不是执行授权；生产 SQLite/UDS/libvirt adapter 随真实执行阶段引入。P1 mock 的设备拓扑由调用方提供，不能替代生产全局设备占用检查。

## 测试结果

工具链：Rust 1.98.1 / Cargo 1.98.1。本轮在受限环境中从已缓存的 crates 离线构建，解包位置 `/tmp/elasticpc-cargo-home`。

```bash
CARGO_HOME=/tmp/elasticpc-cargo-home cargo build -p elasticctl --locked --offline
CARGO_HOME=/tmp/elasticpc-cargo-home cargo fmt --all -- --check
CARGO_HOME=/tmp/elasticpc-cargo-home cargo test --workspace --locked --offline
CARGO_HOME=/tmp/elasticpc-cargo-home cargo clippy --workspace --all-targets --locked --offline -- -D warnings
```

全部 PASS。最终 **66 项测试，0 失败，0 忽略**：core 4、planner 租约/恢复 22、planner 规划 15、probe 单元 5 + fixture 7、CLI 单元 4 + 端到端 9。最终全量测试覆盖所有交叉审查补丁。格式检查通过，Clippy 零警告。

重要断言：挂载/活动 swap 拒绝；整盘/分区双向冲突；组级设备独占；重复身份拒绝；批量租约原子性；重复 start/stop/plan 幂等；六个崩溃位置恢复；丢失 start/stop 响应不误释放；不完整观察保留资源；基底/世代变化拒绝 overlay；未知 OS 不猜；Boot0002 不在 BootOrder 仍保留；DRM 依 canonical PCI 路径；`--dry-run` 计划一致；错误与未知 UUID 也脱敏；参数中的变更命令被拒绝。

CLI 离线验收命令见 [README](../README.md)。两份 golden：

- [Manjaro Trial 计划](../tests/plan-golden/acacia-import-trial.json)：因 root 已挂载、全局占用不完整等条件而阻塞。
- [20 GiB Host 布局计划](../tests/plan-golden/acacia-layout.json)：两个互斥方向均带备份、休眠、离线操作和明确审批门槛。

## 本机硬件发现

证据：[host-2026-10-13-redacted.json](../reports/host-2026-10-13-redacted.json)，命令：

```bash
./target/debug/elasticctl probe --hardware --json --redact
```

这是真实只读扫描，与依据原文重建的 `acacia-fixture-*.json` 分开保存。

- 16 个逻辑 CPU，观察到 x86 虚拟化特性。当前执行环境看不到 `/dev/kvm`；这不等于 CPU 不支持 KVM，不加载模块或提升权限自动修复。
- 系统报告总 RAM **27.21 GiB**，最终快照 `MemAvailable` 约 **18.21 GiB**（随负载变化）。规格的 16+6+2+8 GiB 起点不能直接套用：先按 OS 可见总量及 Host 保留预算调整，核实固件/iGPU 保留原因。
- NVIDIA GPU `0000:01:00.0` 与 HDA `0000:01:00.1` 都属于 IOMMU group **15**，驱动分别是 `nvidia` / `snd_hda_intel`。GPU 暴露 `flr bus`。尚未测试复位稳定性。
- `card0-DP-3` 已连接，归属 NVIDIA `0000:01:00.0`；`card1-eDP-1` 已连接，归属 AMD `0000:65:00.0`。因此当前外屏直出与 Host 内屏是不同 GPU 路径。
- NVIDIA 的瞬时 sysfs 链路读数为 **2.5 GT/s、宽度 x2**。这是当前读数，不代表最大协商能力或负载下带宽，也不能据此断言具体显卡坞连接技术。
- Secure Boot 当前读为禁用。BootCurrent=0006；BootOrder=0006,2002,2003,2001；Boot0002 确实存在而不在 BootOrder。
- 发现 NVMe 的根/ESP 挂载与活动 swap；T7 有 6 个分区。此执行环境未观察到 T7 的可靠 by-id/WWN，规划不能退回临时 `/dev/sda` 身份。
- 已识别 Manjaro；T7 的 Windows OS 分区尚未通过文件证据确认，保留 UnknownInstallation，不擅自挂载。
- `/proc/cmdline` 与 GRUB 含 `resume=`，initramfs 配置含 resume hook，fstab 含 swap。还存在未对应到当前已观察卷身份的 resume 引用，需离线核对。**不能以 swap 不参与休眠为前提执行 Plan A。**

探测仅观察当前可见命名空间；缺失 by-id、KVM 与部分文件可能与执行环境可见性有关。所有这些都作为 gap 保留。没有写真实磁盘、EFI、VFIO；没有启停生产服务或重启。

## 尚未解决

- SQLite 持久事务日志、真实 libvirt/helper/UDS 执行器、跨进程租约及完整 namespace/process/holder 对账。
- owned 文件镜像上的实际 raw + COW 行为与指纹世代机制，随后才是物理系统 Trial。
- Windows OS/BCD/BitLocker/vTPM/驱动与激活；Manjaro 可更新 Boot Bridge 和 Native 回退。
- 正常 Host 环境的 KVM、T7 稳定身份、内存保留、resume 引用和服务数据迁移核验。
- VFIO、Looking Glass、显示/输入切换与独立服务域均未进行真实运行验收。

## 下一阶段需要批准的主机变更

目前只交付规划，没有执行或请求批准具体主机变更。待精确设备身份、备份位置和恢复验证准备好后，再提交可逐项审批的方案：分区/swap/resume、ESP/独立启动项/BootNext、原盘写入、VFIO 绑定、服务迁移与重启。先继续普通文件镜像和隔离介质验证，不把当前条件计划直接变成主机命令。
