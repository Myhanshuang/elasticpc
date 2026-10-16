# 持久控制器集成与主机验证

日期：2026-10-13。

## 已实现和验证

生命周期逻辑现在通过 `LifecycleBackend` 接受不同后端，`DurableController` 使用同一套 SQLite 检查点和跨进程锁。旧 `MockController` / `DurableMockController` 名称保留兼容入口，既有日志格式未变化。

新增 `TestSession` 保留真实 libvirt TEST 连接及本次创建的无盘域，`TestDriverBackend` 将新鲜状态交给持久控制器。后端只接受自身会话的域 UUID、对应安装身份及空资源列表；设备资源请求在写操作日志前被拒绝，恢复带设备租约或其他会话身份的日志同样被拒绝。停止时，即使观察不可用，也不能绕过域和安装身份核对。

`elasticctl lab libvirt-recovery --json` 在临时 SQLite 上启动 service 和 desktop 测试域，在 desktop 启动后注入控制器中断，销毁并重建控制器，再核对状态、重放、停止和清理。`--dry-run` 不创建文件或连接 libvirt；未编译 native feature 时返回 skipped / 退出码 3。实验拒绝 fixture 和生产连接 URI。

必须区分两类故障证据：本实验保留 libvirt TEST 会话，仅重建控制器与 SQLite 连接，**不声称验证整个进程退出后的 libvirt 恢复**。TEST 驱动的状态在进程内共享，不能把并行会话的变化误认为本会话清理失败；清理仅操作本会话 UUID，并核对默认测试域保持不变。真实 QEMU 后端的跨进程恢复仍待实现。[libvirt TEST 驱动](https://libvirt.org/drvtest.html)

新增测试覆盖启动和停止各三个中断边界、未知观察、重复操作不重复调用后端、数据库 revision 冲突后禁止继续操作、模拟后端拒绝真实租约，以及新会话不能冒认旧会话的日志。原有 mock 的物理资源冲突与子进程退出测试继续保留；这些是软件不变量证据，不是实际设备 I/O 释放证明。

## 验收结果

- 默认 workspace：148 项通过，0 失败，3 默认忽略。[日志](../reports/p1-controller-workspace-tests.txt)
- 启用全部 feature：153 项通过，0 失败，3 默认忽略。[日志](../reports/p1-controller-native-tests.txt)
- 格式检查及两种配置的严格 Clippy 通过。[默认 Clippy](../reports/p1-controller-clippy.txt)、[native Clippy](../reports/p1-controller-native-clippy.txt)
- CLI 实验返回 passed，控制器重建和清理成功；明确标记模拟后端、无物理租约支持、未验证整个进程崩溃。[实验报告](../reports/p1-controller-libvirt-recovery.json)

默认忽略的三项仍为 SQLite 子进程入口及两个显式 QEMU 实验；本轮没有为无关代码变更重复启动 TCG Guest。之前的实测见 [Guest 验收](p1-guest-verification.md)。

上轮临时依赖已被清理。本轮重新下载同一 libvirt 包，核对原散列及 Arch 签名，仅在 `/tmp` 解包；没有安装系统包。[依赖恢复记录](../reports/p1-controller-libvirt-runtime.json)

## 主机验证第 1 步：只读检查

当前沙箱报告见 [p1-host-validation-sandbox.json](../reports/p1-host-validation-sandbox.json)：可见 RAM 27.21 GiB，本次 MemAvailable 约 18.26 GiB；内核有 KVM 注册，但没有 `/dev/kvm` 节点。T7 有 udev 别名记录，但本环境不能解析 `/dev/disk/by-id`。这些不是正常 Host 的完整可用性结论。

用户明确批准后，已通过沙箱外执行权限运行以下命令（当前用户，不使用 sudo），退出码 0：

```bash
cd /home/acacia/elasticpc
./target/debug/elasticctl readiness --check-kvm-api --json --redact
```

该命令检查设备路径、内存、挂载、resume 和服务目录元数据，并且只在 `/dev/kvm` 类型/设备号/内核注册均吻合时只读打开，执行 `KVM_GET_API_VERSION`。它不创建虚拟机、不加载模块、不安装包、不写块设备、不改变服务。输出保存到项目报告目录。

批准后的实测见 [p1-host-validation-approved.json](../reports/p1-host-validation-approved.json)：

- `/dev/kvm` 可见，设备核验、只读打开和 `KVM_GET_API_VERSION` 成功，API 版本为 12。以 `kvm_api_probe.api_verified` 为显式探测结论；`kvm` 字段保留默认元数据扫描结果，不代表显式探测失败。
- T7 `/dev/sda` 的唯一稳定 by-id 路径已经解析，报告已脱敏。这仅证明此次快照的身份解析成功，不证明设备无占用或允许写入。
- RAM 共 27.21 GiB，检查时 MemAvailable 约 18.51 GiB。原方案 Guest 24 GiB 加 Host 预留 8 GiB 仍被拒绝。
- resume 引用仍未解析；活动 swap 为 `/dev/nvme0n1p3`。Docker、k3s、containerd 常规数据目录仍落在根分区。
- 固定系统路径中未找到 QEMU/virsh；没有尝试连接主机 libvirt，域和设备占用仍未验证。

本次批准仅覆盖这次离开沙箱的只读检查，已完成；没有修改用户组、设备权限、内核模块或系统服务。KVM API 可用不等于已经验证创建 VM、CPU 扩展或 Guest 执行。

## 第 2 步：隔离 KVM 启动（已授权并通过）

在只读检查确认 KVM API 12、当前用户能访问节点且内存余量充分后，再准备并审查明确的 KVM 实验实现：1 vCPU、256 MiB、独立临时内核/initramfs、无盘、无网络、无共享目录，固定超时和清理。只允许 KVM 加速，不能失败后默默回退 TCG 并宣称 KVM 通过。

用户在只读验收后明确回复“批准，进行测试”，授权本次无盘、无网络、1 vCPU / 256 MiB 的 KVM 启动实验。这是对 AGENTS.md 仅查询 API 边界的本次明确例外，不扩展为生产或原盘操作授权。`guest-boot` 现在默认 TCG，只有显式 `--kvm` 才创建 KVM Guest；保留内存准入、60 秒超时、nonce 串口握手、正常关机与临时文件清理。真实运行的 9 项检查全部通过：PID 1 随机标记、内核 power down、QEMU 退出码 0、无超时及临时文件清理均成立。详见 [KVM 验收](p1-kvm-verification.md)。

## 后续步骤

KVM 无盘验证通过后，先准备独立文件镜像的 libvirt QEMU 生命周期和持久恢复验证，再考虑原盘 Trial。Host libvirt 连接可能触发 socket activation，应明确守护进程与自动启动域的影响后授权。真实安装导入还需要稳定路径解析、完整占用对账、可恢复备份、BitLocker/TPM、resume/swap 以及服务迁移清单。

上述只读检查授权不包含原盘写入、分区、EFI/NVRAM、VFIO/NVIDIA 绑定、生产服务启停或重启。
