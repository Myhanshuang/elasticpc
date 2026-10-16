# 双系统导入、原生回退与保留驱动的 Windows GPU 直通

当前交付是可测试的导入文件工具、Trial 失效保护和交接规划增量，**不是两套真实系统已经导入成功**。此前 Debian 控制台体验版保持独立。没有写真实分区、EFI、NVRAM，没有解绑 GPU、删除驱动、启停生产服务或重启。

## 保留原生驱动的实现选择

- Manjaro 原生 NVIDIA、AMD、HDA、存储驱动和 initramfs 保留。当前原生默认启动入口与 BootOrder 保留。
- Windows 初次 Guest 启动沿用兼容的 SATA/USB 存储路径；确认成功后才在 Trial 写层增量添加 VirtIO。保留原生存储和 NVIDIA 驱动。Guest 独立 OVMF vars 和持久 vTPM 不等同于原生 TPM，仍须确认 BitLocker 恢复条件。
- GPU/HDA 按完整 IOMMU 组交接。运行时交接优先考虑 libvirt managed hostdev；切换的是设备绑定，不删除包，不卸载共享模块，不写全局 NVIDIA blacklist。
- 当前 Manjaro 合成器是否能释放 NVIDIA 未验证。不能释放时拒绝解绑；替代路线是独立 ElasticPC Host 启动配置，在该 Host 内独占 NVIDIA，同时保留原生 Manjaro 配置。此路线仍需独立启动介质或经过批准的 Host 部署。
- 不保证启动时间不变：设备枚举、vTPM、驱动初始化和 GPU 复位都会影响用时。验收分别记录原生和 Guest 冷启动到登录的基线及变更后时间，不用“保留驱动”代替性能实测。

依据：[Linux VFIO 整组所有权规则](https://docs.kernel.org/driver-api/vfio.html)、[libvirt managed PCI hostdev](https://libvirt.org/formatdomain.html#host-device-assignment)。工具能力不证明本机 GPU 能可靠恢复。

## 已实现的入口

### 启动文件检查与暂存

`scripts/boot_bundle.py` 接收明确指定的只读文件树，不自动挂载分区，不修复文件系统。Windows 检查内核与 EFI loader 的 PE 签名、BCD hive 签名；这只是证据收集，**没有解码 BCD 卷映射、检查加密或证明可启动**。Linux 暂存显式选择的内核、initramfs、os-release、fstab 和独立 GRUB 配置。每个文件记录 SHA-256，verify 同时检查源漂移与暂存文件篡改。路径逐级拒绝符号链接；os-release 允许固定的 usr/lib 标准回退。

```sh
python3 scripts/boot_bundle.py inspect --family windows --root /path/to/exported-windows --esp /path/to/exported-esp
python3 scripts/boot_bundle.py stage --family linux --root /path/to/exported-linux --kernel boot/vmlinuz --initrd boot/initramfs.img --root-uuid VERIFIED-FS-UUID --destination target/new-boot-bundle
python3 scripts/boot_bundle.py verify --family linux --root /path/to/exported-linux --kernel boot/vmlinuz --initrd boot/initramfs.img --root-uuid VERIFIED-FS-UUID --destination target/new-boot-bundle
```

产物明确标记 `executable=false`：尚缺真实 EFI loader/FAT 镜像、initramfs 模块检查和 Guest-only fstab/服务覆盖。`noresume` 只禁用恢复，不会阻止 fstab 启用 swap；因此不能仅加这个参数就启动真实根分区。没有删除或重建原生驱动。

### 回退前让文件 Trial 永久失效

```sh
python3 scripts/trial_image.py invalidate-for-native --directory /path/to/stopped-trial
```

该命令只作用于**一个指定的文件 Trial**。独占生命周期锁排除运行中 Guest，持久化标记并 fsync 文件和目录；重试幂等，部分写入也拒绝恢复。现有 preview 和 Trial verify 会拒绝这个档案，必须重新扫描来源并创建新档案。不要手动删除标记继续使用旧 overlay。命令不设置 BootNext、不重启，也没有改变用户默认体验档案。

生产回退仍需全安装档案枚举、全局存储/GPU 占用对账、持久控制器事务和 EFI 执行器；单档案锁不能证明整个物理安装空闲。

### 原生与 GPU 交接规划

```sh
target/debug/elasticctl --json installations list
target/debug/elasticctl --json import native-return --installation INSTALLATION_ID
target/debug/elasticctl --json import gpu-plan --installation WINDOWS_INSTALLATION_ID --gpu 0000:01:00.0
```

Native 计划要求唯一有效原入口、拒绝覆盖已有 BootNext、先让旧世代失效再安排一次性启动，保留 BootOrder。GPU 计划交叉核对 PCI/group 双向成员关系，拒绝漏掉 HDA、重复成员、混入存储等设备、未知原驱动和缺少其他 Host 显示路径。即使拓扑满足，也保留全局占用、合成器释放、复位、管理输入等未验证门禁。两者均不执行物理操作。

## 本机观察与当前阻塞

2026-10-14 重新只读采集：Manjaro 正在使用内盘根分区，原生入口 Boot0006；T7 有 Boot0002，但 OS 卷尚未通过内容/BCD 对应关系确认。不能把第 3 个分区直接宣布为 Windows 安装。

NVIDIA `0000:01:00.0` + HDA `0000:01:00.1` 同属组 15，DP-3 连接 NVIDIA；eDP-1 连接 AMD `0000:65:00.0`。这证明连接拓扑，不证明 NVIDIA 可无中断交接。报告见 `reports/dual-system-host-observation.json`、`dual-system-native-plan.json` 和 `dual-system-gpu-plan.json`。GPU 报告选择的是待识别 T7 候选，因此 Windows 身份门禁正确保持阻塞。

真实 Manjaro 暂存尝试没有完成：`/boot/initramfs-6.12-x86_64.img` 仅 root 可读，沙箱外依然无权读取，`sudo -n` 要求本机密码；没有修改权限或获取密码。所需下一步是用户在本机完成受控只读导出，或提供已导出的启动文件树。当前无独立备份，不推进原盘写入模式。

## 接续顺序与验收

1. 完成受控只读启动文件导出、Windows OS/ESP/BCD/加密与干净关机核对。Windows Fast Startup 也需独立检查，不能从“平时不用休眠”推断已关闭。
2. 先在普通文件副本验证两个 Guest 的启动适配器：Windows 兼容总线；Manjaro 独立 EFI 桥、Guest-only fstab 和服务隔离。两个 Guest 先串行测试，不使用超出实测 RAM 的预算。
3. 从独立 Host 运行，确保原 Manjaro 根完全离线；真实设备 Trial 还需全局占用和不可变基底策略。活动根上的 COW 不能提供一致性。
4. 在原生入口、救援路径与所需备份都验证后，提交具体的 EFI/原盘操作审批；分开验证两系统原生登录与旧 Trial 拒绝。
5. 在 Windows 无 GPU 已可启动后，提供完整组解绑/恢复操作和审批，验证 GPU 驱动、图形、CUDA、反复冷启动与异常恢复。

测试结果分别见 `reports/dual-system-rust-tests.txt`、`dual-system-clippy.txt`、`boot-bundle-tests.txt`、`dual-system-p3-tests.txt`、`dual-system-preview-tests.txt`。软件测试和普通文件实验不计作 P3/P4/P5 实机验收。


后续已在用户本机导出 initramfs 后完成真实只读调查；此前导出阻塞已解除，Windows 卷与 BCD 已取得关联证据，详见 [2026-10-14 实机只读记录](dual-system-live-validation.md)。该记录更新本页的历史阻塞状态。
