# 双系统只读实机检查：2026-10-14

用户批准后执行了原系统只读调查和隔离 EFI 文件实验。没有向原盘写入，没有修改 EFI/BootOrder、原生 initramfs、驱动绑定或生产服务。概览见 [机器报告](../reports/dual-system-live-readonly.json)。

## Manjaro

用户在本机完成仅 root 可读 initramfs 的导出；内核、initramfs、os-release、fstab 的私有副本位于 `.elasticpc/imports/manjaro-readonly`，暂存档案位于 `target/manjaro-boot-bundle`。此前的 sudo 导出阻塞已解除。原生文件没有改变。

initramfs 已包含 `virtio_blk`、`virtio_scsi` 与 ext4 模块，同时仍保留原生 NVMe 与 resume 相关组件。没有为了 Guest 重建或删除原生驱动。

另建 256 MiB FAT EFI 文件镜像，使用 GRUB standalone 加载实际导出的内核和 initramfs。实验固定使用不存在的 root UUID、1 vCPU/1 GiB KVM、私有 OVMF vars、无网络、无真实磁盘/GPU；预期到达“找不到 root”恢复流程。它只验收 EFI→原内核→initramfs，不验收完整 Manjaro 桌面。

实验报告和串口日志：`reports/manjaro-efi-kernel.json`、`reports/manjaro-efi-kernel.log`。第一轮因日志级别未输出版本横幅而超时，原始报告与日志保留在 `*-attempt-1.*`；复测只提高实验镜像的日志级别。复测通过：24.52 秒内加载实际 6.12.108 内核和 initramfs，到达预期缺根恢复流程；QEMU 已回收。完整 OS 仍未启动。

## Windows

使用验签后解压到 `/tmp` 的 NTFS 工具只读查询，没有安装或替换系统 NTFS 包。ESP 中的 BCD、bootmgfw.efi，以及候选 OS 卷中的 ntoskrnl.exe、winload.efi、SYSTEM hive 已复制到 `.elasticpc/imports/windows-readonly`。SYSTEM 副本用于筛选启动驱动与电源配置，不输出其他注册表内容。

- T7 第 3 分区确实存在 Windows 内核和 loader，不再仅靠分区位置猜测。
- BCD 默认对象 `{c862a90d-da03-11f0-a0c4-78465c885d5a}` 的 device/osdevice 数据包含当前 T7 磁盘 GUID 与第 3 分区 PARTUUID，loader 为 `\Windows\system32\winload.efi`。这是实际 GUID 字节关联证据，尚不是通用 BCD 二进制设备解码器。
- `HiberbootEnabled=0`、`HibernateEnabled=0`；hiberfil 查询返回不存在，NTFS volume flags 为 `0x0000`。当前 NTFS 元数据和文件可直接读取，但 BitLocker 管理状态仍需在 Windows 内确认。
- `viostor`、`vioscsi` 未找到；`storahci Start=0` 但 `StartOverride/0=3`；`stornvme Start=0`，没有发现其 StartOverride/0。因此不应不加验证地强制 VirtIO 或假定 SATA 能引导。NVMe 可作为兼容候选，仍需 Guest 实测。
- 原 NVIDIA 驱动存在，保留它；不为实验删除或重装原驱动。

证据分别见 `reports/windows-volume-readonly.json`、`windows-bcd-correlation.json`、`windows-boot-driver-evidence.json`、`windows-shutdown-readonly.json` 和 `windows-boot-bundle.json`。BCD 中还保留旧安装条目，本轮未清理任何条目。

## 阻塞真实双系统体验的条件

1. Manjaro 根目前仍是 Host 正在使用的根，必须从独立 Host 启动后再进行原根导入；不能把活动根作为一致的 Trial 基底。
2. 当前 Windows 卷已用约 258 GiB，本机项目所在文件系统空闲约 61 GiB，不足以完整克隆。需要额外存储，或完成真实只读基底 Trial 执行器与全局占用/不可变性验证；本轮未将物理 T7 接入 Guest。
3. NVIDIA 正被 `llama-server` 使用，Chrome 也持有 NVIDIA 句柄，Xwayland 打开两张 DRM 卡。当前不能直接解绑；需 Windows 无 GPU 先能启动，再安排服务/桌面释放和经过批准的设备交接。当前用户的 fuser 观察也不是全主机占用证明。
4. 原生回退尚需两套系统实际登录、旧 Trial 拒绝、完整 GPU 归还和救援路径验证。原盘写入模式仍缺独立备份。

用户对只读检查和隔离实验的批准已使用，不需重复请求。后续涉及停止上述服务、解绑显卡、改变 EFI 或重启时，需要针对实际操作及恢复步骤确认。

用户随后提供一只可清空但不太稳定的 16 GB U 盘作为候选。设备尚未接入时不猜测设备名、不写盘；须先识别及检查稳定性。它不能容纳 Windows 完整副本，也不能未经验证就作为唯一救援介质。
