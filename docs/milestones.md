# 里程碑与后续门槛

| 阶段 | 本轮交付或下一步 | 真实主机变更 |
|---|---|---|
| P0 | Rust workspace、只读硬件和安装发现、doctor、fixture 与样例 | 无 |
| P1 软件部分 | 导入/布局/资源计划、非执行 DomainSpec、租约与 mock 恢复测试 | 无 |
| P1 持久恢复增量 | 已实现 SQLite 事务检查点、进程互斥、实际子进程退出恢复、普通文件 COW 实测 | 仅自身临时文件 |
| P1 启动与适配器增量 | 无盘 Linux TCG 启动、libvirt test driver 绑定与资源 XML 解析、显式 KVM API 探测 | 自身临时文件、隔离 Guest 进程；test driver 不启动 Guest |
| P1 控制器集成 | 同一 SQLite 控制器接入 libvirt TEST；六处中断边界、未知观察和幂等重放通过 | 无盘测试域与自身临时数据库 |
| P1 KVM 主机验收 | 当前用户通过 KVM API 12 与 1 vCPU / 256 MiB 无盘 Linux 启动、关机、清理 | 已明确授权的隔离 Guest；未变更主机配置 |
| P1 私有真实恢复 | 文件磁盘 KVM Guest、真实控制器 SIGKILL 后恢复、双 Guest 隔离、异常与超时回收 | 已授权独立临时 libvirt 实例与自建文件 |
| P1 实际适配器剩余 | Host QEMU 域观察、生产后端接入、完整占用与路径重核验、UDS 身份策略 | 不自动执行 |
| P2 | Debian 文件镜像构建、独立恢复入口、EFI 卸载演练与[两套布局](p2-host-installation-plan.md)；六阶段隔离验收通过，物理门禁尚未通过 | 分区、swap/resume、ESP/NVRAM 写入须明确批准 |
| P3 | [文件 Trial 档案与 KVM 异常退出重启](p3-file-trial.md)已通过；T7 Trial、Windows 检测、Manjaro Boot Bridge 与生产控制器接入待做 | 真实系统导入需单独计划与批准 |
| P4 | 原盘排他写入、Native 回退、备份恢复与旧 overlay 拒绝 | 原盘写入、BootNext、重启须批准 |
| P5 | 完整 GPU/HDA IOMMU group、复位与 Windows CUDA 实测 | VFIO/NVIDIA 绑定须批准 |
| P6 | Host 显示、Looking Glass、独显直连、输入焦点与救援路径 | 安装/启停 Host 显示组件前提供回退 |
| P7 | 独立服务域、旧服务迁移、产品更新与卸载 | 服务迁移、生产服务启停须批准 |

## 本轮实测对下一阶段的影响

用户说明 AMD 核显在 BIOS 预留 4 GiB；减少预留后仍需重新采集。当前实际 OS 可见总 RAM 约 27.21 GiB，原先 24 GiB Guest + 8 GiB Host 的起点不能直接使用；先调整预算并核实硬件保留。当前发现明确 resume 引用与 initramfs hook，swap 重整方案必须先核对休眠配置。DP-3 已证实连接 NVIDIA，eDP-1 连接 AMD，显示设计按这两条路径推进。详见 [本轮报告](implementation-report.md)。

## 先解决的未知信息

已在用户批准的沙箱外检查中确认 KVM API 12、T7 稳定路径，并通过最小无盘 KVM Guest 启动与关机。先前设备不可见是沙箱观察边界。详见 [KVM 验收](p1-kvm-verification.md)；这些证据不等同于主机全局占用对账或现有系统导入成功。

- 未挂载 Windows 分区的 OS/BCD 对应关系、BitLocker、TPM 与总线驱动兼容性。
- 整台主机各命名空间中的挂载、设备 mapper/RAID、进程句柄与 libvirt 活动域占用；普通用户的局部快照不够。
- 面向现有安装的内核/OVMF/QEMU/swtpm 版本匹配，以及 Guest 固件身份。
- `/etc/fstab`、`resume=`、initramfs/休眠配置及已有 k3s/PV/Docker 的独立迁移清单。
- GPU 实际连接拓扑、可重复复位、显示器 PCI 归属和 Host 保留的救援显示/输入。
- 内容指纹覆盖范围与代价：关键扇区摘要本身并不能证明整个文件系统未在 Native 模式发生变化，需可靠的世代失效流程。

## 尚未要求批准

本轮没有可执行的磁盘/启动/驱动变更方案，不能把概念布局当作具体主机操作审批。下一阶段先完成精确布局、备份位置、恢复验证和分步操作，再提交批准。当前 CLI 的 `requires_approval` 表示未来执行该计划需要批准，不阻止现在只读生成计划。

最新私有真实进程恢复实现及证据见 [验收记录](p1-private-guest-recovery.md)。TEST 驱动与 mock 报告保留为软件层证据。

P2 隔离交付已完成：[验收记录](p2-build-and-verification.md)、[机器报告](../reports/p2-host-integration.json)。完整 P2 实机门禁仍需独立备份、休眠策略、精确布局批准，以及卸载后 Windows/Manjaro 原生启动实测。

## 首个可体验版本

已完成 [本地控制台体验版](first-preview.md)：普通文件 Trial 接入既有 Rust/SQLite 持久控制器，提供 init/start/status/console/stop/recover，实际登录和文件跨重启已通过；16 项真实生命周期检查通过。交付数据与依赖位于 `.elasticpc/`，没有真实磁盘、网络或 GPU 透传。

这完成了“第一个可体验版本”的本轮目标，P3 全部功能仍未完成：Windows 检测、Manjaro EFI Boot Bridge、真实系统导入及实机回退保持待办。

## 双系统与保留驱动交接增量

已加入[启动文件暂存、Trial 原生失效保护、Native/GPU 规划](dual-system-handoff.md)。Windows 文件签名检查不等于 BCD 映射完成，Linux 暂存目录不等于可启动 EFI 桥；P3/P4/P5 的真实系统与显卡验收均未完成。原生驱动包、EFI 与默认启动顺序保持原状。
