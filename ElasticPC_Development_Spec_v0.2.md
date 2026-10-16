# ElasticPC 开发执行规格 v0.2

> 状态：可交给 Codex 直接开始 P0 开发  
> 日期：2026-10-13  
> 目标机器：AMD Phoenix/HawkPoint + AMD 核显 + NVIDIA GeForce RTX 5070 Ti + 32 GB RAM  
> 目标：在个人 PC 上以对等方式管理已有 Windows 与 Linux 安装，保持原生双启动，并在 Elastic 模式下实现客户机常驻、桌面切换和独立服务运行。

## 0. 给 Codex 的执行指令

你是 ElasticPC 项目的实现工程师。将本文视为产品需求、架构基线和分阶段验收合同。

**现在执行 P0（工程骨架与只读硬件发现），并完成 P1 的可在无特权环境测试的虚拟机模型/规划器。** 先检查当前仓库、既有代码与 `AGENTS.md`；不存在项目时创建新项目。不要覆盖已有用户代码。执行后交付代码、测试、样例报告、简明 README 和下一阶段待解决事项，不只输出建议或伪代码。

执行的权限边界：

- **允许**：读取 `/proc`、`/sys`、`/dev/disk/by-*`、`lsblk`、`lspci`、`efibootmgr -v` 等状态；在项目目录内建文件、编译、运行单元测试；在普通文件或 loop 设备上运行模拟测试（仅限明确由测试创建的文件）。
- **本轮不允许**：操作真实分区表、缩分区、格式化、修改 ESP/UEFI NVRAM、改变默认启动项、写入原有 Windows/Manjaro 分区、修改主机 VFIO/NVIDIA 驱动绑定、启停生产服务、重启电脑或插拔设备。对这些操作先输出计划、影响范围、回退步骤，再由用户明确批准后执行。
- 不允许将真实磁盘或真实 PCI 设备路径作为测试写入目标。`--dry-run` 不得调用任何具有写入副作用的接口。
- 优先做端到端可验证的纵向切片；不要一次性创建十几个全是 TODO 的空模块。确实缺少环境依赖时用 mock backend 完成开发、列出缺失项。
- 禁止承诺“所有反作弊游戏均可在 VM 中运行”，也不实现隐藏虚拟机指纹/绕过反作弊的逻辑。
- 输出格式：`实施内容`、`测试结果`、`尚未解决`、`下一阶段需要批准的主机变更`。每项明确到文件路径和测试命令。

## 1. 产品定义与不可改变的约束

### 1.1 核心体验

开机时可选择三种入口：

1. **ElasticPC**：极简专用 Host 启动，分别管理 Windows VM、Manjaro VM、后台服务域。切换当前桌面不重启物理主机，不重启另一客户机。
2. **Windows Native**：原生 Windows 从现有 Samsung PSSD T7 启动，满足需要真实裸机环境的竞技游戏及反作弊程序。
3. **Manjaro Native**：现有 Manjaro 从内置 NVMe 原生启动。

每个系统仍使用**同一套原有系统安装与用户数据**，默认不复制整盘，不要求重装。操作系统安装实例与它的运行模式分离：`Installation` 可以被关联到 `NativeBoot` 和 `ManagedGuest` 两种入口。

在 Elastic 模式中：两台 OS VM 具有相同管理地位，均由控制器管理生命周期；Host 是只承担设备管理/虚拟化的可信管理层，不是用户的日常桌面。服务域生命周期独立于 Windows 和 Manjaro 桌面。

### 1.2 已确定的用户约束

- 必须保留两套现有系统的原生启动，且重装/卸载 ElasticPC 后不能丢失该能力。
- 内屏与外屏都要支持；具备多显示拓扑检测与路由能力，不能假设外屏连接核显。
- 需要竞技网游及反作弊兼容：**原生 Windows 是产品正式支持的使用路径**。部分反作弊明确拒绝虚拟机；不要以 VM 隐藏/规避检测作为实现策略。
- Host 最终优先安装在现有内置 SSD 新分区，不强制额外永久介质；开发验证可暂用 Live USB。
- 内存 32 GB：默认不能让两个 VM 静态分配合计 32 GB；Host、iGPU 共享显存与 I/O 还需要预算。
- CPU/MCP/Web/自动化等后台服务在 Elastic 模式下持续运行；GPU 推理允许暂停、失败重试与重建，不保证运行中的 CUDA 上下文跨模式迁移。
- v0.1 让 RTX 5070 Ti **独占 Windows VM**；不做 GPU 虚拟化共享或 Linux↔Windows 直通热交接。

### 1.3 不在 v0.1 的范围

- 将任意 Windows 游戏、带内核反作弊的程序保证运行在 VM 中；原生启动承担该需求。
- Windows/Manjaro GPU 显存、CUDA 上下文无损迁移；GPU vGPU、SR-IOV 虚拟功能创建。
- Hypervisor/物理 Host 重启时依然保持本机后台服务在线（单台 PC 无法完成）。
- 任意显卡上所有分辨率、HDR、VRR、DRM 内容和高刷新率都同等支持。
- 在用户未批准时自动修复分区、BCD、GRUB、加密分区、TPM 或激活信息。
- 使用旧的保存内存状态恢复一个已在 Native 模式下变更过的磁盘。

## 2. 已确认的硬件事实与待验证项

所有设备地址、容量和状态来自 2026-10-13 用户提供的**只读命令输出**；在实际运行时需重新探测，不能写死。

### 2.1 内置 NVMe：`YMTC YMSS2ED08D25MC`（约 1 TB）

| 分区 | 大小 | 文件系统/用途 | 当前状态 |
|---|---:|---|---|
| `/dev/nvme0n1p1` | 925.3 GiB | ext4，现有 Manjaro `/` | 已挂载；`df -hT` 显示根文件系统约 910 GiB、已用 799 GiB、可用 65 GiB（93%） |
| `/dev/nvme0n1p2` | 299 MiB | FAT/ESP | 挂载 `/boot/efi`，剩余约 267 MiB |
| `/dev/nvme0n1p3` | 28.3 GiB | swap | 正在启用 |

**重要设计后果：** 根文件系统有 65 GiB 可用空间，不等于存在 65 GiB *未分配空间*。当前 GPT 没有 Host 空白分区。新 Host 不能直接安装在所谓的“可用 65 GiB”里；需要另行调整磁盘布局。根分区 ext4 缩小必须在未挂载状态下进行，且要留出实际运行空间。

根分区中有来自 `kubelet` 的挂载和现有服务数据；现有 k3s/Kubernetes 及其他服务的数据必须在迁移方案中单独识别，不能因为 Host 变更就隐式迁走。

### 2.2 外置 Windows 盘：`Samsung PSSD T7`（约 1 TB）

| 分区 | 大小 | 文件系统 | 已确认含义 |
|---|---:|---|---|
| `/dev/sda1` | 260 MiB | FAT/ESP | Windows 外盘 EFI 分区 |
| `/dev/sda2` | 16 MiB | 无 | GPT/MSR 样式分区，继续核验 |
| `/dev/sda3` | 288.7 GiB | NTFS | 具体盘符/OS 卷未确认 |
| `/dev/sda4` | 256.3 GiB | NTFS | 具体用途未确认 |
| `/dev/sda5` | 383.5 GiB | NTFS | 具体用途未确认 |
| `/dev/sda6` | 2 GiB | NTFS | 具体用途未确认 |

Windows 为外盘 Windows 11 To Go 风格安装，原生入口是 `Boot0002`，EFI 位于 T7 的第一个分区。**不要默认 sda3 就是 Windows 系统卷；应只读检查分区内容与 BCD。** Windows To Go 的旧官方功能已被移除，当前这套非标准 Windows 11 外盘安装以实测作为兼容性依据。

### 2.3 UEFI 启动项

- `Boot0006 = Manjaro`，路径 `\EFI\Manjaro\grubx64.efi`，当前 BootCurrent 与 BootOrder 的首项。
- `Boot0002 = EFI USB Device (Samsung PSSD T7)`，当前存在且可作为 Native 启动候选；但**不在当前 BootOrder 中**。不能依据 `BootOrder` 断言其不存在。
- `BootOrder = 0006,2002,2003,2001`，`Timeout = 0`。

ElasticPC 需要创建自己独立的启动项，不覆盖 `Boot0006`、`Boot0002` 和现有 EFI 文件。Native 切换可以规划使用 EFI BootNext；写入 BootNext 属于后续受控操作。

### 2.4 GPU / PCIe / DRM

- NVIDIA RTX 5070 Ti：`0000:01:00.0`，设备 ID `[10de:2c05]`，驱动 `nvidia`，`FLReset+`，可见复位方法 `flr bus`。
- NVIDIA HDA：`0000:01:00.1`，当前 `snd_hda_intel`。
- 两者**同属一个 IOMMU Group**，且已输出的组成员仅这两个功能；直通时作为一组处理。
- GPU 在 `00:01.2 → 01:00.[0,1]` 分支；USB4 桥 `00:04.1 → [06-64]` 在本次输出未显示下游显卡。**不能把 GPU 按已确认的 USB4 隧道设备处理**；显卡坞具体物理连接方式及链路速率仍需检测。
- AMD HawkPoint 核显：`0000:65:00.0`，当前 `amdgpu`。Host 保留其显示控制权。
- 当前有两个 `connected` DRM 接口：`card0-DP-3` 和 `card1-eDP-1`。**card0/card1 与物理 GPU 的映射尚未核实**，必须用 `/sys/class/drm/cardX/device` 的真实路径解析；不要基于编号猜测。
- `sriov_totalvfs = 1`、`Number of VFs = 0`，未暴露 mdev 类型；v0.1 不使用 SR-IOV/vGPU。

### 2.5 尚未获取的能力

CPU 的 `svm`/KVM 可用状态（此前 `lscpu` 受本地化输出影响）、Host Secure Boot 状态、实际物理可用内存、Windows 是否 BitLocker 加密、Manjaro 的 `/etc/fstab`/initramfs/恢复配置、Host 显示连接器的厂商映射、外接 GPU 实际 `LnkSta` 宽度/速率，以及 swap 是否用于 hibernation/resume。

这些全部属于 P0 的**只读检测**，有结果之后才允许形成 P2 的磁盘变更计划。

## 3. 体系结构

```text
UEFI Boot Manager
  ├── Boot0006: Manjaro Native  ──> 现有内置 NVMe 的 Manjaro
  ├── Boot0002: Windows Native  ──> 现有 Samsung T7 的 Windows
  └── ElasticPC Boot Entry
         │
         ▼
  ElasticPC Minimal Linux Host（独立系统分区）
         ├── Linux kernel + KVM / VFIO / DRM-KMS
         ├── QEMU / libvirt / OVMF / swtpm
         ├── Elastic Controller
         │     ├── Hardware & OS Discovery
         │     ├── Boot / Storage Adapter
         │     ├── VM Reconciler
         │     ├── Device Lease Broker
         │     ├── Display & Input Broker
         │     └── Recovery / Power Policy
         ├── Service Domain（独立生命周期）
         │     └── Web / MCP / SSH / Agent API / GPU API 网关
         ├── Windows VM（T7 现有系统，物理盘源）
         │     └── NVIDIA RTX 5070 Ti VFIO（GPU + HDA）
         └── Manjaro VM（内置 NVMe 现有 ext4 分区）
               └── VirtIO-GPU 2D，后续 virgl/Venus 3D

  Host DRM/Wayland ──> 内屏 eDP、Host 控制的外屏
  Windows GPU     ──> NVIDIA 直连外屏 / Looking Glass 画面回传
```

### 3.1 底层决定

- **第一版 Hypervisor**：Linux KVM + QEMU + libvirt 的 `qemu:///system`；不 fork KVM/QEMU/Proxmox。
- **Host 基础发行版**：优先 Debian Minimal，固定经验证的内核/虚拟化软件版本；不在 Host 安装 KDE/GNOME。必要时使用打包好的最小 Wayland compositor 承担桌面切换。
- **控制器语言**：Rust，`tokio`、`serde`、`tracing`、`clap`；持久状态优先 SQLite；libvirt 使用 `virt` (libvirt-rust) 绑定，隔离在 backend adapter 中。依赖版本由 Cargo.lock 固定，不使用假设的 API。
- **本地接口**：Unix Domain Socket + 本地 CLI；图形管理 UI 后置。Unix socket 基于 peer credentials/polkit 类策略控制权限。
- **VM 固件**：Q35 + OVMF，Windows VM 有独立 OVMF 变量存储和稳定的 vTPM 状态。
- **Host 更新**：后续实现版本化镜像与可回退启动；初期只在开发环境产出可启动镜像，不触碰实际 boot order。
- **预留 Xen Backend 接口但不实现 Xen**。不要构建没有需求的通用抽象层；在接口上保留 backend capabilities 即可。

### 3.2 组件边界

| 组件 | 权责 | 不应负责的事 |
|---|---|---|
| `hardware-discovery` | 从内核/sysfs/UEFI 得到设备、IOMMU、GPU、存储、接口拓扑与能力 | 不更改设备绑定 |
| `installation-discovery` | 只读发现 Windows/Linux 实例、ESP、系统卷、启动方法 | 不自动修复分区和启动 |
| `boot-planner` | 生成 Native 或 Guest 启动计划，维护启动档案 | 不擅自修改用户原生 EFI/BCD/GRUB |
| `storage-broker` | 锁定整盘/分区、验证挂载互斥、COW/原盘模式和基底指纹 | 不允许多个写者 |
| `hypervisor-backend` | 创建/启停/观察 VM、固件配置、guest agent 事件 | 不推断硬件资源策略 |
| `device-broker` | 分配 IOMMU Group/USB 等租约、VFIO 准入检查 | 不为两个 VM 同时分配同组设备 |
| `display-broker` | 选择画面路径、焦点、内外屏、键鼠音频 | 不假装能通过 Guest GPU 驱动 Host 不可访问的显示端口 |
| `service-manager` | 管理 Service Domain、依赖与健康检查、GPU API 路由 | 不把桌面关闭视为服务关闭 |
| `controller` | 驱动目标状态与状态机、跨模块协调及恢复 | 不直接用 shell 拼接危险管理命令 |

### 3.3 基本数据模型

至少应有以下强类型实体（JSON 序列化，Rust enum 有明确版本/未知值策略）：

```rust
struct Installation {
    id: InstallationId,
    family: OsFamily,              // Windows | Linux | Unknown
    display_name: String,
    source: InstallationSource,     // PhysicalDisk | PhysicalPartitions | Image
    volumes: Vec<VolumeRef>,
    boot_profiles: Vec<BootProfile>,// Native | ManagedGuest
    fingerprint: SourceFingerprint,
    compatibility: CompatibilityReport,
}

struct ManagedDomain {
    id: DomainId,
    installation_id: InstallationId,
    backend: HypervisorBackendId,
    cpu: CpuProfile,
    memory: MemoryProfile,
    devices: Vec<DeviceRequest>,
    storage_policy: StoragePolicy,
    display_profile: DisplayProfile,
    desired_state: DesiredState,
}

enum StoragePolicy {
    InspectReadOnly,
    TrialOverlay { overlay_id: String },
    NativeBackedExclusive,
}

enum LeaseResource {
    BlockDisk(DiskStableId),
    BlockPartition(PartUuid),
    PciIommuGroup(String),
    UsbDevice(UsbStableId),
    DisplayConnector(ConnectorId),
}

enum CapabilityStatus { Supported, Experimental, Unsupported, Unknown }
```

以上是设计合同，可调整 Rust 具体类型，但**对上层保持安装实例、启动模式、物理资源租约相互独立**的语义。

`BlockDisk` 租约覆盖其所有分区：如果 Windows VM 租用了 T7 整盘，不允许另外的 Domain 或 Host 在其任一分区取得写入租约。`BlockPartition` 租约按物理 GPT PARTUUID 管理，Host 自己占用的 Host 根分区始终为排他资源。锁管理不仅靠内存中的 HashMap，还要在故障重启后从真实挂载、进程、libvirt 和内核句柄状态重新核对。

### 3.4 Controller 状态机与幂等性

```text
Discovered → Planned → Validated → Leasing → Attaching → Running
                                     │                      │
                                     └──> Rollback <── Failed
Running → GracefulShutdown → Detaching → LeaseReleased → Stopped
```

- 每次切换操作记录 `operation_id`、`installation_id`、`desired_state`、前置条件、已获取的租约、执行步骤和补偿步骤。
- `plan` 与 `apply` 分开；`plan` 只读且幂等，`apply` 需要权限并复核所有前置条件。
- Controller 重启后从 libvirt/sysfs/mounts 恢复实际状态，不能凭旧 SQLite 记录直接重新分配 GPU 或磁盘。
- 发现真实设备变化（T7 被拔出、BDF 变化、显示器切换）时，重新发现硬件并更新能力，不沿用旧路径。
- 失败时释放尚未交付给 VM 的资源；对已被 QEMU 持有的块设备，先核实 VM 退出/已停止 I/O，再允许再分配。

## 4. Boot 与已有系统导入

### 4.1 发现与导入流程

```text
Probe disks/EFI → Detect installation → Build import plan
                 → Offline boot feasibility checks
                 → Trial overlay boot
                 → Guest driver/firmware verification
                 → Native-backed exclusive activation
```

**发现器不能仅靠 FSTYPE 猜测 OS。** Linux 只读识别 `/etc/os-release`、`/etc/fstab`、`/boot`、`/boot/efi`、内核/initramfs；Windows 只读识别 `Windows/System32`、BCD/EFI、NTFS 卷信息和 GPT 属性，必要时挂载只读。加密卷无法自动解密时标记 `NeedsUserInput`。

不将 Bare-metal 的 NVRAM 内容直接映射为 VM 的 NVRAM；每个 Guest 固件有独立持久的 OVMF 变量文件。系统盘与 EFI 的关系由导入计划描述。

### 4.2 Windows T7：整盘为主、试运行为先

- 保留原盘 GPT 和 ESP；首次导入以 T7 **原盘只读基底 + 独立写入覆盖层**试验虚拟引导。基底原盘必须未挂载可写，试验前后校验指定关键扇区/ESP/分区元信息哈希。
- 可用 QEMU 的 qcow2 backing chain 机制，但必须先在 loop/文件镜像中验证 raw block backing 的实际行为以及只读约束，不能假定 `qemu-img` 参数在所有版本和块设备上通用。
- 虚拟存储控制器选择优先保证已有 Windows 启动驱动兼容，先尝试被现有系统认识的 SATA/USB 存储路径，再导入 VirtIO 驱动，不在第一次启动就强制换到 virtio-blk。
- Guest 使用独立的 OVMF vars/vTPM；识别并处理 BitLocker/Device Encryption 恢复要求。Native 与 Guest 的硬件身份、TPM/UEFI 必然不同；记录可能发生的驱动和激活变化。
- `NativeBackedExclusive` 阶段才允许对原盘写入。保存 GPT+ESP+BCD 的可恢复备份，关注 Guest Windows 更新是否修改物理 ESP/BCD。保留裸机启动不是“禁止系统更新 EFI”，而是要在更新后验证 Native 启动仍可恢复。
- 优先 block-device mapping（Host 不得读写挂载）；如果 Windows To Go 风格安装因磁盘总线变化无法引导，作为第二适配器测试完整 USB 存储设备直通；不要默认两者表现相同。
- T7 拔出或重连时，`/dev/sda` 可能变化；使用 stable ID/USB 设备身份并重新核验大小、GPT disk GUID 与分区 PARTUUID。

### 4.3 Manjaro：共享 NVMe 的分区级导入

**禁止**把内置整个 `/dev/nvme0n1` 暴露为 Manjaro Guest 的可写物理磁盘。Host 与 Guest 共享该 NVMe 控制器，只能给 Guest 分配原来的 Manjaro 分区及明确批准的其他分区。

初步 Guest 磁盘结构：

```text
Manjaro VM
  ├── Guest 专用虚拟 EFI/boot 磁盘（不覆盖原生 ESP）
  ├── PhysicalPartition(nvme0n1p1)，作为 Guest ext4 根数据源
  ├── 独立虚拟 Swap（不得共享 Host 正在使用的 nvme0n1p3）
  └── VirtIO/Emulated storage + VirtIO network + VirtIO GPU
```

- 初期可使用 QEMU 直接加载 Linux kernel+initramfs 验证 rootfs 映射及虚拟化驱动，不把这种研发启动方式当作最终 EFI 体验。
- 正式方案实现 `EFI Boot Bridge`，保留可更新的 Guest 引导档案，跟踪原生内核/initramfs 更新；可通过独立 guest ESP/bootloader 实现。不能把某一次 kernel/initramfs 复制后长期不更新。
- Guest 识别同一 ext4 根 UUID/PARTUUID；使用 `fstab`、内核 `root=` 等原有语义。针对 swap、裸机网卡等 VM 不存在的硬件，提供可逆的 Guest 专属配置（DMI/虚拟化条件化或 Guest overlay），不破坏 Native 启动。
- Host 不得挂载 Guest root 可写，不得从宿主机运行依赖原有 Manjaro root 的生产服务并同时让 Guest 写它。原本位于 `nvme0n1p1` 的 k3s/MCP 等数据要在未来单独迁移到 Service Domain 的专属存储。
- 原生系统更新内核、GRUB 或 EFI 后，Import Scanner 要能检出启动配置漂移、刷新 Guest Boot Bridge。

### 4.4 COW / Raw 正式模式的切换

- `InspectReadOnly` → `TrialOverlay` 不修改原盘。
- `TrialOverlay` 的测试写入只在独立覆盖层。覆盖层明确绑定底盘指纹（总大小、GPT GUID、选定区域散列/世代）；基底在 Native 模式被写入后，旧覆盖层**禁止恢复**，只能重新生成或受控合并。
- `NativeBackedExclusive` 允许原盘写入，但进入前必须完成完整关机、持久化块缓存、确认 Host/其他 VM 无其他写入者。转换时不能带着 Trial overlay 的运行中内存快照。
- Native 与 ManagedGuest 切换时必须关机/重启物理 PC；从 Elastic Mode 切到 Native 前**关闭对应 VM，并终止全部对同一磁盘的写入**。
- 第一次直接写原盘的实验需有可实际验证的原生启动恢复备份。

## 5. 安装策略：当前内置 SSD 没有空白分区

优先最终在内置 SSD 建立 16–24 GiB 左右的极简 Host 根分区（具体根据 Host 镜像实际占用调整），EFI boot entry 独立。**先不执行分区变更。** P1 只产出按当前布局计算的可审查计划，P2 才从 Live/离线环境安装。

Plan A：核实当前 `nvme0n1p3` 28.3 GiB swap 是否承担休眠/`resume=`。如果**不需要休眠且用户同意减少/取消磁盘 swap**，可考虑将该分区空间重整为 Host + 适当的 swap；Manjaro `fstab`、`resume` 及 initramfs 相关引用必须同步调整并验证 Native 启动。

Plan B：如果必须保留完整 swap，则先清理或转移根分区数据，保证缩分区完成后仍有充足 ext4 可用空间；然后从 Live 环境离线检查并缩小 ext4、调整 GPT，再建 Host 分区。`df` 当前“可用 65 GiB”不是空闲分区，不能在线缩减已挂载 ext4 根分区。

建议优先为 Host 引导目录设置独立 `\EFI\ElasticPC\...`，不覆盖 `\EFI\Manjaro\grubx64.efi`。现有 ESP 只有 299 MiB，必须计算 boot 文件实际空间；必要时另设 Host 的 EFI 分区。所有实际分区与 EFI 操作都要求完整备份、分步确认与原生引导回退验证。

开发初期可在 Live USB/隔离介质上验证 Host 镜像和 GPU/显示，不将“永久外置 Host 磁盘”强加为最终产品结构。

## 6. 设备与 GPU 管理

### 6.1 GPU 初版规则

- Host 驱动 AMD 核显，Windows VM 独占 NVIDIA `01:00.0` + HDA `01:00.1` 的完整 IOMMU Group。
- `flr bus` 说明有内核暴露的复位方法；不等于复位百分百稳定。首次做 VFIO 直通前测试设备实际复位、驱动回收、连续 VM 重启。
- 初版在专用 Host 中使用可预测的启动早期设备绑定，避免 NVIDIA 驱动抢占；这是 P5 的真实主机改动，须在恢复路径已验证后执行。
- GPU 直通在 VM 和用户眼中按完整物理显卡处理；Windows CUDA 推理服务随 Windows 运行，Linux/Service VM 通过 API 调用。不安排 Linux Guest 同时加载 NVIDIA 原生驱动访问该物理 GPU。
- 设备选择时按 Vendor/Device ID + PCIe 拓扑/IOMMU Group/复位能力复核。BDF `01:00.0` 仅是本次探测快照。

### 6.2 Device Broker 与租约

`Reserve → Revalidate → DetachHostDriver → AttachToGuest → VerifyGuest → Release → Rebind/Reset`。执行应记录步骤与失败原因，不允许同一 IOMMU Group 跨 VM 分配。host 显示 GPU 永远不允许无救援通道地转移。

VFIO 能否直通，还受主板固件/IOMMU、GPU 音频功能、QEMU 设备拓扑及 Guest 驱动影响。发现能力报告要按 `Supported / Experimental / Unsupported / Unknown` 评级，附具体缺口。

## 7. 显示链路与输入

### 7.1 两类显示路径

**Host-controlled path：** AMD iGPU 的 DRM/KMS + 最小 Wayland compositor 管理内屏 `eDP` 及接在 Host GPU 的外屏。Manjaro VM 以 VirtIO-GPU 显示；Windows VM 的画面通过 Looking Glass 抓取/共享内存回传给 Host，在内屏或 Host 外屏显示。

**Guest-direct path：** Windows VM 独占的 NVIDIA 直出连接在 NVIDIA 上的外屏，适合游戏。切换到 Manjaro 时，如果外屏只有 NVIDIA 一根线，Host 无法直接驱动该显示接口：UI 必须提示改用 Host 输出、显示器另一个输入源，或者降级为通过 Windows 内的远程观看器显示 Linux 桌面（该降级依赖 Windows 运行）。

明确区分“所有屏幕至少有可用路径”和“所有屏幕任意 VM 无缝零拷贝直出”两个目标。v0.1 只承诺前者在已支持的连线拓扑上成立。

### 7.2 Display Broker 接口

- 探测 `/sys/class/drm/*/device`、connector status、EDID（不在日志中默认输出序列号）、可用模式、GPU 归属。
- `list-outputs / list-sources / route / focus / audio-route / restore-last`。
- 支持内屏、Host 外屏、Windows 直连外屏、多输入显示器的自动/手动模式。
- 输入由 Host 统一捕获并路由；全局回退组合键由 Host 持有，Windows/Manjaro 不能劫持该组合键。直通 USB 外设时保留可用的管理输入。
- Looking Glass 需验证 ivshmem/KVMFR、Windows 显示输出（物理/虚拟 IDD）、Windows 捕获模式与带宽；不能默认无显示输出时能够自动捕获桌面。

### 7.3 迭代顺序

1. 在虚拟窗口正常显示 Manjaro/Windows（不依赖 dGPU）。
2. 在 Host iGPU 内屏显示 Windows Looking Glass。
3. Windows 直通 GPU 外屏显示。
4. 内外屏混合、热插拔、鼠标/键盘/音频焦点、分辨率。
5. 可选 3D acceleration、HDR、VRR 等增强能力。

## 8. 服务与 RAM 预算

### 8.1 Service Domain

Service Domain 拥有独立的虚拟磁盘、网络与生命周期；可以重启任意桌面 VM 而不停止它。第一阶段先运行 CPU/MCP/SSH/Web 服务，GPU API 网关是稳定入口（可继续公开 `:5070`），Windows 内的 CUDA/llama.cpp 是可消失、可恢复的后端。

后端下线时，网关返回结构化 `backend_unavailable` 或对有界、可重试请求排队，绝不能把“网关仍能连接”等同于“运行中的推理请求没有中断”。

原来保存在 Manjaro 根分区的 k3s/PV/Docker 或其他状态需要迁移清单；Service Domain 不能并发挂载原来 Manjaro Guest 的可写根分区。

### 8.2 32GB 内存分配建议（非静态总是满配）

- **双桌面配置起点**：Windows 16 GiB + Manjaro 6 GiB + Service Domain 2 GiB，给 Host/核显/缓存和额外开销保留约 8 GiB 预算。根据实际 `MemAvailable` 和 workloads 调整。
- **游戏配置**：暂停/关闭 Manjaro，Windows 可以扩大到约 20 GiB，Service Domain 继续运行；更新 VM 内存前检查 Guest 支持，不默认所有 VM 可无损在线缩容。
- GPU 直通 Windows 默认不依赖内存 ballooning 做临时内存回收；优先正常关机/受控暂停非前台 VM 和定制配置文件。
- 不保证 Native Windows 运行期间本机 Service Domain 依然在线：进入原生 Windows 会关闭 Elastic Host。确实需要这项能力则需另一台常开设备。

## 9. 源码结构与 API 合同

### 9.1 P0 项目结构（先做薄的纵向切片）

```text
elasticpc/
├── Cargo.toml                 # Rust workspace
├── Cargo.lock
├── README.md
├── docs/
│   ├── architecture.md
│   ├── milestones.md
│   └── hardware-fixture-notes.md
├── crates/
│   ├── elastic-core/          # 类型、能力、计划、状态
│   ├── elastic-probe/         # 只读硬件/系统扫描
│   ├── elastic-planner/       # import/boot/resource 纯规划器
│   └── elasticctl/            # CLI（probe, doctor, plan）
├── fixtures/
│   ├── acacia-redacted/      # 匿名化的用户机器输出及预期解析
│   └── generic/              # AMD/Intel/缺 GPU/双 SSD 等模拟输入
└── tests/
    ├── probe-fixtures/
    ├── safety-invariants/
    └── plan-golden/
```

P1 后新增 `elastic-virt-libvirt`、`elastic-controller`、`elastic-device`、`elastic-display`、`elastic-services`、`elastic-helper`、`elastic-host-image` 等包。不要在 P0 为尚无实现的模块创建空占位 crate。

### 9.2 CLI（最初就稳定）

```text
elasticctl doctor [--json]
elasticctl probe --hardware [--json] [--redact]
elasticctl probe --installations [--json] [--redact]
elasticctl installations list
elasticctl import plan --installation <id> --mode trial [--json]
elasticctl layout plan --host-size 20GiB [--json]

# 后续实现；P0 只保留 CLI/API 设计，不直接访问物理设备执行：
elasticctl import apply --plan <id>
elasticctl vm start|stop|status <id>
elasticctl display list|switch <id>
elasticctl boot native --target <windows|manjaro>
```

所有计划必须输出：`preconditions`、`affected_devices`、`operations`、`requires_approval`、`rollback`、`capability_gaps`。Plan 存储为 schema-versioned JSON，后续变更可迁移；路径中的 block device 采用稳定标识。

后续本地 RPC 候选：`GET /v1/capabilities`、`GET /v1/installations`、`POST /v1/import-plans`、`POST /v1/domains/{id}/start`、`POST /v1/display/routes`。优先 UDS，之后通过本机 UI 客户端使用；不默认监听公共 TCP。

### 9.3 代码质量/权限原则

- Rust 严格区分纯扫描/规划与具有副作用的执行：`Probe` 不要求 root；缺少读取权限时标记具体未知，不用权限提升自动补救。
- 后续有写入需要时由受限 `privileged-helper` 执行白名单动作；上层 UI/插件不具备任意 shell/root 权限。
- 不执行从用户输入拼接的 shell 命令。命令行调用使用固定可执行路径和参数数组，解析机器可读 JSON，强制 `LC_ALL=C` 防止中文区域化输出改变字段。
- 敏感硬件序列号、磁盘 ID、用户路径在 telemetry/fixtures 默认脱敏；本机稳定设备身份可在本地数据库保留。
- 事件驱动与幂等性优先，系统崩溃后重建设备租约/VM 状态。
- 测试环境不得要求用户 root 才能运行全部单元测试；涉及特权/硬件的集成测试单独标记。

## 10. 分阶段开发任务、验收与交付

### P0：仓库、能力发现、安装目录（Codex 现在执行）

**交付**：可编译 Rust Workspace；`elasticctl doctor`、`elasticctl probe --hardware --json`、`elasticctl probe --installations --json`；硬件/安装实例数据模型；匿名化当前硬件 fixture；Golden 测试；README。

能力发现至少覆盖：

- CPU arch/虚拟化支持、`/dev/kvm`、IOMMU、RAM、内核、Secure Boot 信息（可未知）。
- PCIe GPU/HDA 的 Vendor/Device、现有驱动、IOMMU Group、reset methods、SR-IOV/mdev 可见性。
- 物理磁盘容量、接口、GPT/分区、文件系统、mountpoints、stable IDs、swap/休眠引用。
- EFI BootCurrent/BootOrder/Boot#### 并正确处理 Boot#### 不在 BootOrder 的情况。
- DRM 的 card→PCI 设备真实映射、显示接口连接状态；支持无显示器机器。
- Windows/Linux 只读发现；不能识别时返回 `UnknownInstallation` 而不是臆测。
- `doctor` 根据实测区分 `known-supported` / `missing-prerequisite` / `needs-verification`。

**验收测试**：

1. `cargo fmt --all -- --check`；`cargo test --workspace`；`cargo clippy --workspace --all-targets -- -D warnings`。
2. Fixture 解析结果准确：GPU + HDA 同一组，`flr/bus`、现有 Manjaro root、T7 ESP、`Boot0002` 虽不在 BootOrder 仍可发现；DRM card0/card1 不凭编号猜测。
3. 识别到 `nvme0n1p1` 仍挂载时，任何 writable Guest 计划报冲突；识别到 `/dev/sda` USB 盘时只用 stable ID。
4. 所有输出机器可读；probe 在缺 `lspci`/`efibootmgr`/权限时不 panic，报告 degraded capabilities。
5. `P0` 不写真实设备、不要求修改 EFI 或驱动。测试具备非 root 离线模式。

### P1：VM 规划器、租约与 mock libvirt（Codex 可接着实现非特权部分）

**交付**：`ImportPlan`/`ResourcePlan`、纯规划式 libvirt domain spec，`MockHypervisor`，状态机和 lease manager；基于 test:///default 或 mock 的虚拟机生命周期测试。

**验收**：同一物理分区两个写租约冲突；整个 T7 与任一子分区租约冲突；GPU IOMMU group 不能同时分配；重复 `start/stop/plan` 幂等；模拟控制器在中间步骤崩溃后恢复一致状态；规划器不对真实物理设备调用写操作。

### P2：最小 Host 启动镜像与安装方案（物理改动前审批）

**交付**：Debian Minimal Host 构建、启动/恢复入口、Host 分区布局两套规划、UEFI Boot Entry 计划、Live/隔离介质验证记录。

**门禁**：检查 swap/resume，确定分区空间与备份后才允许操作真实 SSD；严格保留原生入口；检验卸载 ElasticPC 后两套原生系统可启动。

### P3：原有系统 Trial 导入（先 T7，再 Manjaro）

**交付**：全盘/分区只读底层 + COW 写层、Windows 安装检测、Manjaro EFI Boot Bridge、Windows/Manjaro 的 Guest 固件档案、基底指纹验证。

**验收**：无须复制整块 1TB 数据也可完成 Trial 启动；退出后原盘关键区域哈希不变；变更原盘后旧 Trial overlay 拒绝恢复；两个 Guest 均能启动到登录界面。

### P4：Native-backed 排他模式与 Native 回退

**交付**：持久块设备租约、Native-backed 切换流程、UEFI BootNext 计划/执行器、双向启动验证、故障回滚。

**验收**：同一安装既能虚拟启动又能原生启动、使用同一文件；两种模式切换无并发写；不尝试把已变更基底配上旧内存快照恢复；发生导入失败仍可由 UEFI 独立进入原系统。

### P5：RTX 5070 Ti VFIO 与 Windows 图形/CUDA

**交付**：GPU+HDA 同组直通、VM Q35/OVMF/TPM 配置、重启/复位测试、服务端 CUDA 测试。

**验收**：Windows 识别真实 GPU、NVIDIA 驱动可工作；基本图形负载及 CUDA 程序可运行；VM 连续关机/重启/重新启动的 GPU 可靠性有可复现记录。

### P6：Display Broker 与内外屏切换

**交付**：Host Wayland shell、VirtIO-GPU/Linux 显示、Looking Glass/Windows 内屏显示、NVIDIA 外屏直显、输入与音频焦点切换；不同显示拓扑的支持矩阵。

**验收**：两个运行中 VM 可切换焦点且保留各自应用；内屏与外屏按可用连线路径正确显示；断开外屏后内屏仍可进入 Host 救援界面。

### P7：独立服务域、资源策略、恢复与产品化

**交付**：Service VM、旧服务迁移向导、GPU API 网关、状态恢复、Installer/Uninstaller、版本更新回退、第二台机器兼容性验证。

**验收**：重启任一桌面 Guest 时 Service VM 的 CPU 服务持续可用；Windows CUDA 后端重启后网关恢复；完整卸载不破坏原生启动项。

## 11. 专项测试清单

### 存储安全

- [ ] Guest root 分区已经被 Host 可写挂载时拒绝租约。
- [ ] 两个 Guest 不能对相同 PARTUUID 同时请求可写权限。
- [ ] Windows VM 请求整个 T7 后，任何其他进程不能被控制器授权写入 T7 子分区。
- [ ] 只读基底 + COW 试运行退出后，原始镜像完全不变。
- [ ] Native 模式改动原始磁盘后，旧覆盖层与旧内存快照均不能恢复。
- [ ] Host EFI、Manjaro EFI、Windows EFI 三种启动路径没有错误覆盖。
- [ ] 盘符变化、UUID 与 GPT 变更能触发重新评估，而非误用 `/dev/sda`。

### VM/资源

- [ ] `start`、`stop` 幂等；服务/控制器崩溃后状态可恢复。
- [ ] IOMMU Group 租约而非单 PCI Function 租约；GPU/HDA 不拆分所有权。
- [ ] 宿主始终有足够 CPU/RAM 和可用的 Host GPU/输入设备。
- [ ] T7 被拔出，控制器对数据 I/O/VM 停止状态报告清楚。

### 显示/服务

- [ ] 内置 eDP 由 Host 驱动，可显示救援 UI。
- [ ] 外屏不同物理接线拓扑下给出明确路径/降级原因。
- [ ] 双 VM 切换不重启其他 VM；后台 CPU 服务不中断。
- [ ] 需裸机反作弊游戏时能选择 Windows Native，并明确本机 Host 服务暂停。

## 12. P0 当轮交付格式

Codex 完成第一轮后按下面结构汇报：

```text
Implementation:
- created/modified: <files>
- implemented commands: <commands>
- architecture decisions: <only concrete deviations from this spec>

Verification:
- cargo fmt ... : PASS/FAIL
- cargo test ... : PASS/FAIL
- cargo clippy ... : PASS/FAIL
- fixture invariants: PASS/FAIL

Hardware findings:
- confirmed facts and unknowns from read-only probes
- no write operations on physical disks / EFI / VFIO

Next:
- P1 software-only implementation items
- host changes awaiting explicit authorization
```

## 13. 开发资料与适用范围

本节仅列已核实的原始文档和它们对应的能力，不延伸为对你这台设备已通过实测的断言。

1. libvirt Domain XML（物理块设备、hostdev、OVMF）：https://libvirt.org/formatdomain.html
2. Linux VFIO/IOMMU Group 文档：https://docs.kernel.org/driver-api/vfio.html
3. libvirt 磁盘镜像 backing chains：https://www.libvirt.org/kbase/backing_chains.html
4. libvirt virtlockd 与锁的局限：https://www.libvirt.org/kbase/locking-lockd.html
5. QEMU VirtIO GPU（2D/virgl/Venus）：https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html
6. Looking Glass 要求与显示链路：https://looking-glass.io/docs/B7/requirements/
7. libvirt-rust 绑定：https://github.com/libvirt/libvirt-rust
8. `resize2fs` 关于离线缩小 ext4：https://man7.org/linux/man-pages/man8/resize2fs.8.html
9. `efibootmgr` BootNext：https://manpages.debian.org/unstable/efibootmgr/efibootmgr.8.en.html
10. 微软 Windows To Go 官方生命周期：https://learn.microsoft.com/en-us/windows/deployment/planning/windows-to-go-overview
11. Riot 反作弊对虚拟化环境的官方说明（2026-07-16）：https://support.riotgames.com/en-us/riot/client/error-van-9100

---

**执行优先级总结：** P0 先完成可用工具，准确理解这台机器；P1 建立能力/租约/规划的可测试抽象；只有在两个阶段都稳定后，才允许进行 Host 分区改造、物理系统导入和 GPU 驱动直通。ElasticPC 成功与否以“现有系统的双模可启动、分区不损坏、两桌面可切换、后台服务独立”为最终验收，而不以是否有漂亮的管理 UI 为验收。
