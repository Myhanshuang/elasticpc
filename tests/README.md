# 验收索引

可执行 Rust 测试放在各 crate 的 `src`/`tests` 中，`cargo test --workspace --locked --offline` 统一执行。本目录存放跨模块验收契约与 golden 数据。

- `crates/elastic-probe/tests`：同一解析路径下的硬件/EFI/DRM/安装 fixture、退化观察。
- `crates/elastic-planner/tests`：导入/布局/DomainSpec、内存预算、覆盖层指纹、租约和故障恢复。
- `crates/elasticctl/tests/cli.rs`：隔离 PATH 的 CLI 端到端验收，JSON、错误、redact、dry-run 与计划 golden。
- `crates/elastic-state`：真实 SQLite 提交前/后进程退出、CAS、未知 schema 与安全路径。
- `crates/elastic-controller/tests/durability.rs`：持久意图/租约、跨进程互斥、启动/停止六个崩溃点与存储失败。
- `crates/elastic-lab`：测试拥有的文件、工具约束、超时和缺依赖逻辑；实际 QEMU 集成需显式运行。
- `crates/elastic-lab/src/guest.rs`：无盘 Linux（默认 TCG，显式 KVM）、静态 PID 1、nonce 串口握手与关机、内存门禁和临时文件清理。
- `crates/elastic-virt-libvirt`：未知/不完整 XML 资源保留未知、URI 限定；启用 native feature 后测试真实 test driver API。
- `crates/elastic-core/src/redact.rs`：脱敏一致性及未知 schema/枚举策略。
- `plan-golden/*.json`：从匿名 fixture 得到的可审查计划，包含阻塞条件。更新需先审阅差异，不能用自动覆盖来掩盖失败。

测试不会创建 loop 设备、打开真实块设备写入、触发 VFIO 或修改 EFI。mock 结果仅证明软件不变量，不能证明真实硬件可用。

真实 QEMU 集成（选择已验签工具目录）：

```bash
ELASTICPC_TEST_QEMU_TOOL_DIR=/tmp/elasticpc-qemu-tools/usr/bin cargo test -p elastic-lab --locked --offline real_qemu_file_cow_lab -- --ignored --nocapture
```

默认被标为 ignored 的 SQLite 崩溃子进程入口由父测试显式调用；不代表 SQLite 崩溃路径未测试。无盘 Guest 启动也要求显式执行，详见 [本轮复现说明](../docs/p1-guest-and-libvirt.md)。native libvirt 测试通过只证明测试驱动 API，不代表真实 Guest 启动或 Host 设备可分配。

KVM 实测必须使用显式 `guest-boot --kvm` 并获得主机测试授权；默认测试不会打开 KVM。固定参数、无回退、内存/非 root 门禁与 dry-run 由普通测试覆盖，实际验收见 [KVM 验收](../docs/p1-kvm-verification.md)。

最新文件 COW 与恢复复验见 [复验报告](../docs/p1-file-retest.md)：全 feature 157 项通过，真实文件 I/O 与模拟域恢复分开记录。

私有真实 Guest/SQLite 进程恢复验收见 [报告](../docs/p1-private-guest-recovery.md)。默认 `make check` 不启动虚拟机；`make private-lab` 仅在授权后显式执行。

## P2 Host

`python3 -m unittest discover -s tests/p2 -v` 检查构建器 dry-run、内存门禁、超时回收与启动证据判定。隔离 KVM/UEFI 实测见 [P2 验收流程](../docs/p2-build-and-verification.md)。不对真实 Windows/Manjaro 回退作推断。

## P3 文件 Trial

`make p3-check` 覆盖普通文件输入门禁、同尺寸变更、跨进程锁和失败清理。临时 QEMU 依赖存在时增加真实 raw/COW 隔离与 backing chain 拒绝测试，否则明确跳过这两项。真实 KVM 结果见 [P3 报告](../reports/p3-file-trial.json)。

## 首个体验版

`make preview-check` 覆盖监督连接未知状态、私有目录、参数路径与跨进程继承锁。`scripts/test_experience.py <全新测试目录>` 为显式授权的真实 KVM 交互验收：CLI 控制台、文件跨重启、控制器 AfterStart/AfterStop SIGKILL、监督进程退出回收、过期底盘拒绝。末尾故意修改测试目录的底盘副本，因此不能把测试目录作为用户体验档案。
