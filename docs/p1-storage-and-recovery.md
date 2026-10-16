# P1 持久日志与普通文件 COW 验证

日期：2026-10-13。本轮是在 P0/P1 软件模型之上的增量，不代表物理系统导入已通过。

## 实施内容

新增三个有实际实现的 crate：

- `elastic-state`：SQLite 单检查点表、应用/schema 版本、事务 revision CAS、严格 journal JSON 校验、普通文件与 sidecar 路径检查。
- `elastic-controller`：在整个生命周期操作期间持有跨进程文件锁，将 mock controller 的每个检查点同步提交到 SQLite；提供端到端恢复实验。
- `elastic-lab`：仅创建自身拥有的私有目录和 8 MiB raw 文件，使用真实 `qemu-img` / `qemu-io` 验证 qcow2 写入、整文件指纹和过期基底拒绝。

既有 `MockController` 增加可选同步 `JournalSink`。意图与获取的租约先提交，再调用模拟后端；后端返回之后再次提交。任何日志错误都会使该控制器停止接受后续生命周期操作，必须从持久状态和新的后端观察重建。停止时也不能吞掉日志错误。没有安装 journal sink 的旧纯内存测试保持原行为。

SQLite 选择 `journal_mode=DELETE` 与 `synchronous=EXTRA`。EXTRA 在 FULL 的基础上加强回滚日志删除后的目录同步；实际保证仍取决于文件系统与存储硬件遵守同步语义。CAS 防止覆盖其他写者的 revision，额外的进程锁负责隔离生命周期副作用。[SQLite 同步策略](https://www.sqlite.org/pragma.html#pragma_synchronous) · [原子提交](https://www.sqlite.org/atomiccommit.html) · [事务](https://www.sqlite.org/lang_transaction.html)

控制器正常销毁时由持锁进程显式解锁，避免并发创建的子进程临时继承描述符而延长旧锁寿命。锁记录获取时的 PID，继承句柄的子进程销毁自身副本不会释放父进程的锁；两种情况均有确定性回归测试。[Linux flock 语义](https://man7.org/linux/man-pages/man2/flock.2.html)

数据库不会自动初始化一个已经存在但不认识的文件。设备、符号链接、硬链接、其他用户可写父目录、危险 sidecar、文件替换、未知版本或损坏数据都拒绝。该机制协调合作的 ElasticPC 进程，不声称能约束任意同用户恶意进程或其他 Host 设备写者。

## 新命令

```bash
./target/debug/elasticctl readiness --json --redact
./target/debug/elasticctl --fixture fixtures/acacia-redacted readiness --json --redact
./target/debug/elasticctl lab recovery --json
./target/debug/elasticctl lab file-cow --json
./target/debug/elasticctl lab file-cow --tool-dir /absolute/verified/qemu/bin --json
./target/debug/elasticctl --dry-run lab recovery --json
./target/debug/elasticctl --dry-run lab file-cow --json
```

`readiness` 只读；两个 `lab` 命令明确写入自己新建的临时文件，完成后清理。它们不接受用户给定的底盘/覆盖层路径。`--tool-dir` 仅选择固定名称的 QEMU 可执行文件，调用方需先验证其来源；不提供任意命令、参数或 shell 脚本接口。`--fixture` 不允许用于实验，避免把模拟输入当作实测证据。

`--dry-run` 只生成操作说明及依赖元数据，不创建目录、数据库、镜像或运行 QEMU。实验退出码：通过/仅规划为 0；失败/拒绝为 1；缺依赖未运行为 3。所有状态仍以单个 JSON 对象返回。

## 实际 QEMU 证据

主机系统路径没有 QEMU 工具。本轮从 [Arch 官方 qemu-img 包入口](https://archlinux.org/packages/extra/x86_64/qemu-img/)下载 `11.1.2-1`，使用本机已有 Arch 密钥验证签名，仅将 `qemu-img` 与 `qemu-io` 解包到 `/tmp/elasticpc-qemu-tools/usr/bin`。没有运行软件包安装脚本，也没有修改系统目录或启动服务。

包和签名依据见 [qemu-tools-provenance.json](../reports/qemu-tools-provenance.json)。临时工具目录在系统清理 `/tmp` 后可能消失；后续可使用已安装工具，或重新验证后解包。没有将第三方二进制放进源码目录。

[实测报告](../reports/p1-file-cow-lab.json)包含实际命令、QEMU 版本、退出码、校验值与清理结果。验证步骤：

1. 创建有已知字节内容的 raw 测试文件，记录整文件 SHA-256，移除其写权限。
2. 明确使用 `-f qcow2 -F raw -b` 创建独立覆盖层；不依赖 backing format 猜测。
3. 检查未写区域从底盘继承；在覆盖层写入已知字节，再分别读取覆盖层与 raw 文件核对。
4. 验证整个底盘散列不变、覆盖层内容变化，运行 `qemu-img check`。
5. QEMU 关闭后受控改变测试底盘，确认绑定的旧指纹拒绝恢复；变更后不再打开旧覆盖层运行 QEMU。
6. 删除所有本次生成的测试镜像和私有目录。

语法依据：[QEMU qemu-img](https://www.qemu.org/docs/master/tools/qemu-img.html) · [QEMU qemu-io 命令源码](https://gitlab.com/qemu-project/qemu/-/blob/master/qemu-io-cmds.c)。

这是实际的 QEMU 块 I/O 测试，**没有启动 Windows/Linux Guest OS，没有验证 KVM、真实块设备 backing、OVMF/vTPM 或 libvirt**。小文件的整盘 SHA-256 也不能直接当作 1 TB 原盘的生产指纹方案。

## 恢复验证

[SQLite 实验报告](../reports/p1-sqlite-recovery-lab.json)明确区分 `real_sqlite_with_mock_backend`：元数据真实落盘，Guest 和设备是模拟对象。实例包含 service + desktop，恢复后拒绝冲突磁盘请求，停止 desktop 不停止 service，重复操作不重复调用后端。

测试还在独立进程中提前退出，验证 SQLite 提交前回滚和提交后保留；在控制器关键边界退出并恢复；强制终止持锁进程，确认旧锁随进程释放；注入提交失败和 revision 冲突，确认后续后端操作被拒绝。进程崩溃测试不等于硬件断电测试。详细命令与最终统计见 [验收记录](p1-verification.md)。

## 本机 readiness 结论

证据：[p1-host-readiness-redacted.json](../reports/p1-host-readiness-redacted.json)。

- 当前执行环境不见 `/dev/kvm`，未执行 KVM ioctl 或加载内核模块。
- 可见总 RAM 27.21 GiB；原规格 24 GiB Guest + 8 GiB Host 的预算被规划器拒绝。
- T7 能观察到 serial 字段，但没有可核对的 by-id/WWN；不能将 serial 字符串直接当作已经验证的执行身份。
- 当前内核与 GRUB 的 resume 目标在可见分区拓扑中无法解析，可能过期或不可见；不是“可以删除 swap”的证据。
- Docker、k3s、containerd 的常规目录存在，其底层位于 `/dev/nvme0n1p1`。只观察目录/挂载元数据，没有读取服务数据内容，也没有把该清单视为完整迁移清单。
- 系统未检测到 virsh。默认不连接 libvirt socket，因为 socket activation 可启动守护进程，继而影响自动启动对象；`--readonly` 本身不能保证这种连接没有主机副作用。[libvirt 守护进程说明](https://www.libvirt.org/daemons.html)

本轮提供离线 libvirt UUID 清单解析，未实现或冒充完整的实际设备占用适配器。

## 下一阶段

先补齐正常 Host 环境的稳定磁盘身份、KVM 访问、resume 对应关系和服务迁移位置，再进行隔离文件 Guest 的启动验证与 libvirt adapter。生产域、持久租约与真实 I/O 状态仍需完整对账。真实分区/swap、ESP/NVRAM、原盘写入、VFIO、服务启停和重启仍分别提交具体操作及回退方案后批准。
