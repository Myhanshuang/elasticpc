# 架构与本轮设计复核

本文记录 v0.2 规格在 P0/P1 的落地决定。原规格仍是产品合同。所有真实设备写操作留在后续审批阶段。

## 1. 先把观察、判断与执行分开

`elastic-probe` 只读取已存在的文件、sysfs 链接和白名单命令输出；`elastic-core` 保存版本化事实；`elastic-planner` 只接受这些值，返回计划。`elasticctl` 提供发现、诊断和计划命令，没有 `apply`、物理 VM 启停、EFI 或 VFIO 执行器。

报告中的 `Supported` 只支持对应的窄命题。例如有 `svm` 表示观察到 CPU 特性，不代表已成功启动 KVM。复位方法存在不等于复位可靠。Secure Boot 的 Supported/Unsupported 表示已启用/已禁用，不代表产品兼容性结论。缺失工具、不可读取的数据与未执行的试验明确标为 Unknown 或 Experimental。

P0 不自动挂载未挂载分区，也不打开真实块设备计算内容指纹。这使普通用户可以安全运行，但未挂载的 NTFS 卷不能凭类型识别为 Windows。需要后续经批准的只读离线识别。已挂载的 Linux/Windows 文件证据用于识别；安装实例与 Native/Guest 启动档案保持分离。

## 2. 资源保护必须覆盖祖先关系

租约管理器显式建模 `DiskStableId → PARTUUID`。整盘租约与任何子分区冲突，不能通过换 `/dev/sdX` 名称绕过。Host 根分区、活动 swap 和未知占用不能被当成可分配空闲空间。PCI 资源按完整 IOMMU Group 分配；显示 card 编号不具有设备身份语义。

Linux VFIO 文档把 IOMMU group 作为隔离与所有权边界，而不只是 PCI function；这支持规格的 GPU+HDA 同组策略。[Linux VFIO 文档](https://docs.kernel.org/driver-api/vfio.html)

libvirt 的 virtlockd 使用建议锁；这不能强制阻止任意 Host 进程或挂载访问同一设备。因此后续必须结合挂载、swap、block holders、进程、命名空间、libvirt 实态与执行前复核。P0 的普通用户扫描不能宣称已完成全局占用核验，`storage_observation_complete` 默认为 false。[libvirt virtlockd](https://www.libvirt.org/kbase/locking-lockd.html)

## 3. Trial 仍然要求底盘静止

COW 隔离 Guest 写入不等于能与持续变化的原盘安全共存。规划器必须拒绝 Host 正在写的基底，并在下一阶段为 overlay 固定磁盘大小、GPT/PARTUUID、关键内容散列与世代。只有拓扑信息时不能批准旧 overlay 或内存快照恢复。

将来生成 qcow2 时必须显式指定 backing format；不能依靠自动探测。libvirt 的存储权限管理需要准确的 backing chain。当前 DomainSpec 是待检查的结构化输入，`executable=false`，不输出貌似可直接执行但缺少实测前提的 XML。[libvirt backing chains](https://www.libvirt.org/kbase/backing_chains.html) · [Domain XML](https://libvirt.org/formatdomain.html)

每台 Guest 的 OVMF vars、vTPM、Linux EFI Boot Bridge 分别管理；不复制裸机 NVRAM，不把 Host 使用的整块 NVMe 暴露为 Guest 可写磁盘。

## 4. 恢复按观察结果重建

P1 mock backend 本身在内存里工作，没有打开设备、启动 Guest 进程或管理真实 libvirt 的能力。操作日志支持 JSON 序列化，并已通过 JournalSink 接入 SQLite 持久检查点。恢复时以模拟后端和 Host 的新观察为准；旧日志本身不授权重新分配资源。不确定后端是否仍持有 I/O 时保留租约，停止失败不能冒充资源已释放。

第一轮先验证状态机；后续已实现 SQLite 事务检查点和进程互斥，详见 [P1 持久恢复](p1-storage-and-recovery.md)。随后新增官方 libvirt Rust 绑定、限定 `test:///default` 的生命周期验证与保守 XML 观察，另用 TCG 实测无盘 Linux 启动，详见 [启动与适配器](p1-guest-and-libvirt.md)。现在同一个持久控制器通过 `LifecycleBackend` 接入 mock 与无盘 TEST 后端，保留旧 API 别名及日志格式。TEST 后端拒绝所有设备租约与非本会话域，不能用模拟完整性释放真实资源。Host QEMU 连接、生产后端、受限 helper 和 UDS daemon 仍待实现。详情见 [集成及主机验证计划](p1-controller-host-validation.md)。

## 5. 布局是条件方案，不是自动命令脚本

`layout plan` 提供 swap 空间重整与 ext4 离线缩减两个条件方向。文件系统内部可用容量不作为 GPT 未分配空间；不知道分区边界时也不编造可用区域。取消 swap 必须确认休眠/恢复引用，改 ext4 必须离线并验证恢复备份。[resize2fs](https://man7.org/linux/man-pages/man8/resize2fs.8.html)

内存规划同时检查物理总量、当前 MemAvailable 与 Host 保留预算。暂停 VM 不自动释放其 RAM，不依赖 Windows VFIO Guest 在线缩容来维持安全预算。

## 6. 版本与隐私边界

JSON `schema_version=1`。不支持的 schema 必须拒绝；观察枚举可降级 Unknown，未知执行动作不能静默变成已支持动作。计划 ID 由内容派生，便于相同输入重复规划；它不是执行授权，也不能代替重探测。

`--redact` 生成仅供展示的报告，替换设备身份、卷 UUID、安装 ID、EDID 摘要和用户路径，同时保留引用关系。脱敏输出不能用于操作真实设备。fixtures 使用虚构 ID，不收集遥测。原始 live 报告如有必要由用户保存在本地，仓库样例仅保留脱敏输出。
