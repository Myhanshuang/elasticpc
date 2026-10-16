# P1 增量验收记录

日期：2026-10-13。执行目录：`/home/acacia/elasticpc`。测试以普通用户运行，未安装系统软件包，未操作真实磁盘写入、EFI、VFIO、生产服务或重启。

## 全量检查

本次使用已有离线依赖缓存 `CARGO_HOME=/tmp/elasticpc-cargo-home`；该路径是当前机器的临时构建环境，不是项目运行依赖。

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 格式 | `cargo fmt --all -- --check` | 通过 |
| Workspace 测试 | `cargo test --workspace --locked --offline` | 114 通过，0 失败，2 默认忽略 |
| Clippy | `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过 |
| CLI 构建 | `cargo build -p elasticctl --locked --offline` | 通过 |

原始输出：[测试日志](../reports/p1-workspace-tests.txt)、[Clippy 日志](../reports/p1-clippy.txt)。

| 模块 | 通过测试数 |
| --- | ---: |
| elastic-core | 4 |
| elastic-probe | 23 |
| elastic-planner | 37 |
| elastic-state | 9 |
| elastic-controller | 14 |
| elastic-lab | 9 |
| elasticctl | 18 |
| 合计 | 114 |

两个默认忽略的入口有不同用途：SQLite 子进程入口由父测试主动调用，以测试提交前后的突然退出；QEMU 文件实验要求显式指定已验证的真实工具，本轮另行执行并通过。不能用默认测试通过推断 QEMU 已执行。

## 真实文件实验

使用已验签的 QEMU 11.1.2 工具，显式集成测试通过：

```bash
CARGO_HOME=/tmp/elasticpc-cargo-home \
ELASTICPC_TEST_QEMU_TOOL_DIR=/tmp/elasticpc-qemu-tools/usr/bin \
cargo test -p elastic-lab --offline real_qemu_file_cow_lab -- --ignored --nocapture
```

CLI 实验的完整证据单独保存在 [p1-file-cow-lab.json](../reports/p1-file-cow-lab.json)：14 项检查通过，8 个实际 QEMU 命令均退出 0，没有命令超时。底盘写前与覆盖层写后 SHA-256 相同；受控改变临时底盘后散列不同，旧覆盖层被指纹检查拒绝。所有实验镜像及其私有目录已清理。工具包来源、散列及验签结果见 [工具来源记录](../reports/qemu-tools-provenance.json)。

这项验证使用 8 MiB 普通文件，验证 QEMU 块 I/O 与本实验的指纹规则；没有启动 Guest OS，也没有验证物理块设备或 KVM。

## 持久恢复与并发

最终构建执行 `./target/debug/elasticctl lab recovery --json`，结果为 `passed`、`real_sqlite_with_mock_backend`，持久 revision 为 24，临时数据库清理成功。报告见 [p1-sqlite-recovery-lab.json](../reports/p1-sqlite-recovery-lab.json)。

测试覆盖 SQLite 原子提交、跨进程写入竞争、控制器启动与停止各三个崩溃边界、SIGKILL 后重新获取锁、提交失败后的拒绝执行，以及未知观察下不释放资源。生命周期文件锁在所属进程销毁控制器时显式释放；子进程继承句柄后销毁自身副本不会解锁父进程。两项确定性回归测试覆盖这些文件描述符语义。

后端仍是 mock。进程退出测试不等同于断电测试，实际持久性仍依赖文件系统和硬件同步语义。

## 主机检查与后续门槛

[只读 readiness 报告](../reports/p1-host-readiness-redacted.json)保留本轮可见环境的结论：未见 `/dev/kvm`；可见 RAM 27.21 GiB，规格的 24 GiB Guest + 8 GiB Host 预算被拒绝；T7 尚缺经过核对的 by-id/WWN；resume 目标在可见拓扑中未解析；若干服务数据目录位于当前 Host 根分区。

因此本轮完成持久元数据、恢复机制和普通文件 COW 验证。Guest 启动、libvirt 实际状态对账和物理导入仍属于后续阶段。实施范围与下一步见 [P1 实施报告](p1-storage-and-recovery.md)。
