# 私有文件磁盘与真实进程恢复验收

日期：2026-10-13。用户批准整套 1–11 项测试、独立临时 libvirt 实例，以及测试后的项目构建推进。

## 实施内容

新增 `elastic-controller::private_libvirt_lab`，复用既有 `DurableController`、SQLite 检查点、revision CAS 和进程互斥。专用 `private_controller` 进程只操作实验 manifest 中的两个 UUID，连接显式的私有 Unix socket，禁用自动启动连接；不接受生产 URI或物理资源声明。实验停止操作使用 libvirt destroy；正常 Guest 关机另以串口指令和 pidfd 验证。这仍是实验后端，不是生产调度器或优雅停机接口。

`private_guest_lab.py` 创建私有 XDG 配置、运行、缓存和数据目录，自行传递 socket activation 文件描述符，启动普通用户的临时 virtqemud，不调用 systemctl。空实例核验通过后才定义两个固定 1 vCPU / 256 MiB、无网卡、无共享目录或直通设备的域。每次实际启动前重新检查内存，保留至少 8 GiB Host 预算及两个 Guest 的内存和开销余量。

新 PID 1 从 initramfs 内加载 **Guest** virtio-blk 模块，向本次创建的 qcow2 文件磁盘写入随机标记、fsync、读回，并在下一次启动验证持久化。模块不会加载到 Host。8 MiB raw 基底保持只读，Guest 只连接覆盖层。运行前后比较完整基底 SHA-256，停止后还用 QEMU 转换为普通 raw 文件核对数据。

运行时恢复工具 `prepare_lab_runtime.py` 固定包版本和散列、验证 Arch 签名，仅在私有目录解包。QEMU/libvirt 工具使用私有 RPATH。发行包的 CPU 型号表路径被固定长度重定位到另一个私有临时目录，记录原始与派生散列；不修改 `/usr`。该处理只适用于当前固定版本，匹配不成立即拒绝。见 [依赖记录](../reports/p1-private-runtime-provenance.json)。

实现参考：[libvirt daemon 与 XDG 路径](https://libvirt.org/manpages/virtqemud.html)、[显式连接 URI](https://libvirt.org/uri.html)、[QMP query-kvm](https://www.qemu.org/docs/master/interop/qmp-spec.html)。

## 验收清单与证据范围

| 批准项目 | 验证方式 |
|---|---|
| 1. Guest 文件磁盘读写 | 真实 KVM Guest 写入、重启读回，停止后 Host 转换镜像核对标记 |
| 2. 基底隔离 | Guest I/O 前后 raw 全文件 SHA-256 相同 |
| 3. 生命周期与重放 | 多次真实启动/停止；重复操作 ID 不再创建或停止域 |
| 4. 控制器进程崩溃 | Rust 进程在持久化边界被 SIGKILL，另一个进程打开原 SQLite 恢复 |
| 5. Guest 异常退出 | pidfd 定位本次 UUID 与 QEMU 可执行文件，SIGKILL 后观察退出并对账 |
| 6. 六处中断 | AfterJournal、AfterLeases、AfterStart、AfterStopIntent、AfterStop、AfterRelease 均在真实实例运行 |
| 7. 冲突和未知 | 真实 QEMU 镜像写锁、两个控制器 SQLite 锁、私有 socket 暂时不可达；revision 冲突另由 workspace 故障测试覆盖 |
| 8. 旧覆盖层拒绝 | 所有 Guest 停止后只改变可丢弃基底，Rust 后端拒绝启动旧覆盖层 |
| 9. 双 Guest 隔离 | desktop 停止、控制器崩溃、Guest 崩溃和超时回收期间 service 保持运行 |
| 10. 超时与清理 | 注入 1 秒测试截止，pidfd 确认实验进程被回收；删除全部实验域后结束 daemon 并删除目录 |
| 11. 软件回归 | 默认 155 项、全部 feature 160 项通过，均 0 失败、3 默认忽略；Python 2 项通过，格式和两种配置严格 Clippy 通过 |

最终实测 **45 项检查全部通过**，临时 daemon 退出码 0，实验目录已删除，并另行核对不存在残留实验目录。真实运行报告见 [最终集成报告](../reports/p1-private-guest-integration.json)。报告保留每个 Rust 控制器子进程的返回值和六次 SIGKILL 证据；正常关机同时要求串口 poweroff 标记、内核 power down 与 pidfd 退出事件。例行运行状态检查间隔至少 150 秒；串口、pidfd 和子进程完成使用事件驱动等待。完整实验另设 240 秒截止，命令与串口分别有超时。

[默认测试](../reports/p1-private-default-tests.txt) · [全 feature 测试](../reports/p1-private-workspace-tests.txt) · [Python 测试](../reports/p1-private-harness-tests.txt) · [默认 Clippy](../reports/p1-private-clippy.txt) · [native Clippy](../reports/p1-private-native-clippy.txt)

## 构建与复现

项目增加 Makefile。默认检查不启动 KVM 或 daemon，真实实验有单独的显式入口：

```bash
# 已准备临时依赖和 Cargo 缓存时
CARGO_HOME=/tmp/elasticpc-integration-cargo make check
CARGO_HOME=/tmp/elasticpc-integration-cargo make check-native
make private-lab-dry-run

# 仅在已有主机实验授权的普通用户环境执行
CARGO_HOME=/tmp/elasticpc-integration-cargo make private-lab \
  LAB_RUNTIME=/tmp/elasticpc-integration-runtime \
  LAB_REPORT=reports/private-lab-latest.json
```

运行时已清理时，执行以下显式下载准备步骤。目标目录必须不存在；不会覆盖已有目录或安装系统包：

```bash
/usr/bin/python3 scripts/prepare_lab_runtime.py \
  --destination /tmp/elasticpc-integration-runtime
```

脚本要求带 pidfd 支持的 `/usr/bin/python3`、当前机器的 `/boot/vmlinuz-6.12-x86_64` 和匹配 virtio-blk 模块。它是此主机的研发验收工具，不能直接当成通用安装器。工具缓存和验签材料保留，Guest、实验 daemon 和每次测试目录均须清理；清理不能确认时保留现场并以失败报告返回。

## 尚未解决与下一阶段

控制器退出后恢复的是已有 Guest 的管理状态，不是 Guest 内存快照恢复，也不证明 Host 断电后的硬件写缓存持久性。本次仅使用实验新建的文件磁盘；物理设备租约、全主机占用对账、现有 Windows/Manjaro、UEFI/vTPM、BitLocker、GPU 和服务迁移仍未验证。

测试通过后已将临时验证入口整理进 controller crate，补充统一构建入口、软件测试和可复现依赖准备。下一阶段应把实验后端收敛为受控生产适配器：完整 XML/镜像链核验、持久文件资源身份、优雅停机策略、后台执行身份与权限隔离。真实磁盘、EFI、驱动绑定和生产服务变更仍需具体方案与单独批准。
