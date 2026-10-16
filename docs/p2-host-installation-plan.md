# P2 Host 镜像与物理安装方案

P2 分为隔离交付和物理部署门禁。镜像只在自建 qcow2 和独立 OVMF 变量文件内构建、启动与卸载。Windows/Manjaro 原生启动、真实 EFI 回退和分区变更尚未验收，不以虚拟机中的 Debian 回退替代。

## 镜像设计

以 Debian 13 官方 genericcloud 镜像为最小基础，无图形桌面；固定基础镜像版本，离线安装 QEMU、libvirt 和 elasticctl。基础镜像按官方 HTTPS SHA512 清单校验；Debian 包索引使用固定 Debian 13 archive 公钥验签，再逐级校验 Packages 与 .deb。两种认证强度在报告中分别记录。

构建机只运行一台 1 vCPU / 1 GiB KVM Guest，无网络、无真实磁盘透传。启动前要求 MemAvailable 至少 9472 MiB，保留 8 GiB 和 256 MiB 开销。输出为没有 backing file 的 qcow2 与独立 OVMF 变量文件；验收用另一个 overlay，避免卸载测试破坏交付镜像。

正常入口运行只读 doctor 并验证系统 libvirt 可连接。恢复入口通过 GRUB 独立 systemd target 执行只读 readiness；目前是串口诊断入口，尚非可交互的桌面救援环境。镜像没有预置口令和 SSH 登录权限，部署前需另行设计操作者身份与控制台。此镜像用于 P2 隔离验收，不直接覆盖现有机器。

## 两套 SSD 布局（计划，不执行）

| 项目 | A：复用约 28.3 GiB swap 空间 | B：保留 swap，离线缩小 Manjaro ext4 |
|---|---|---|
| Host 根分区 | 20 GiB ext4 | 至少 20 GiB ext4，从经验证的可缩小空间分配 |
| Host ESP | 优先独立 1 GiB FAT32；剩余空间是否回作 swap 按实际扇区计算 | 新建独立 1 GiB FAT32；需一并预留空间 |
| 原有根分区 | 保持边界 | 需备份、Live 环境离线检查及缩小文件系统后再改分区边界 |
| 原有 swap/resume | 已发现 resume 引用，当前禁止删除或调整 | 完整保留；仍需确认其真实目标与休眠可用性 |
| 前置条件 | 用户明确无需休眠或已有经过验证的替代方案；同步修订 fstab、resume 和 initramfs | 得到实际最小文件系统大小、使用量、精确扇区和安全余量；禁止在线缩小 |
| 失败回退 | 恢复分区表、swap 身份与启动配置 | 从独立备份恢复文件系统和分区表；仅恢复 GPT 不能撤销文件系统缩小 |

两方案都保留 Manjaro 根、原有 ESP 和 T7 Windows 所有分区。不能仅按设备名或历史容量生成写命令：实施前按 WWN/序列号、PARTUUID、起止 LBA 和文件系统 UUID 复核。当前没有可批准的实际扇区表，CLI layout plan 仍只供评估。

用户说明 AMD 核显在 BIOS 预留 4 GiB，可考虑减少；这属于用户提供的硬件解释。资源预算仍依据实测 27.21 GiB。修改 BIOS 后必须重新采集，不能预支将来可能释放的内存。是否保留休眠也不能从可用 RAM 推断。

## UEFI 入口与回退

1. 备份 GPT 主/备表、ESP 完整文件树和哈希、`efibootmgr -v`、BootOrder、fstab、内核参数和 initramfs 配置到独立介质。
2. 为 Host 单独使用 `EFI/ElasticPC`，不覆盖 Microsoft、Manjaro、Debian 或 fallback `EFI/BOOT`。真实安装必须记录新建文件与 Boot#### 的所有权；同名已有入口应报冲突。
3. 首次试启动使用一次性 BootNext 或固件菜单；验证成功前不永久替换原 BootOrder。失败由固件菜单进入已验证的原生系统或独立救援介质。
4. 卸载前验证所有权和原始 EFI 哈希，只删除本项目创建的入口及文件。恢复 BootOrder 时合并保留期间新增的其他入口，不盲目覆写整个变量库。
5. 分别实际启动 Windows 和 Manjaro 到登录界面，记录时间与磁盘身份；完成卸载后再次启动两者。隔离镜像中保留 Debian EFI 的测试只证明实现模式，不满足这项实机门禁。

## 备份与部署待确认

- 用户是否需要休眠；现有 resume 引用是否与实际 swap 一致。
- 独立备份介质或目录、可用容量、文件系统和恢复工具。备份不能仅放在将要调整的 SSD 上。
- Manjaro 数据及服务卷备份清单，Windows 恢复密钥已由用户安全保管（不向项目日志提交密钥）。至少完成抽样恢复，关键数据需完整校验。
- 可启动救援介质及实际启动证据；两套原生系统在改动前均可启动。

这些信息齐备后才能形成精确扇区布局、逐步操作和回退记录并提交真实 SSD/EFI 变更审批。用户对隔离实验的授权不等于对真实分区操作授权。

本次实际只读生成的 [脱敏布局计划](../reports/p2-layout-plan-redacted.json)确认 swap 区为 30,411,849,728 字节，当前仍 active；Manjaro 根分区仍挂载。两者分别阻止直接复用 swap 和在线缩小根分区。此报告中的脱敏 ID 只用于审阅，执行前必须重采真实稳定身份。
