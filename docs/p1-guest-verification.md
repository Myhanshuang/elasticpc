# Linux 启动与 libvirt 验收记录

日期：2026-10-13。执行目录：`/home/acacia/elasticpc`，普通用户；Rust 1.98.1。检查使用固定 Cargo.lock 和离线缓存 `/tmp/elasticpc-next-cargo-home`。

## 完整检查

| 检查 | 结果 | 原始证据 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 通过 | 无格式差异 |
| `cargo test --workspace --locked --offline` | 146 通过，0 失败，3 默认忽略 | [默认测试日志](../reports/p1-guest-workspace-tests.txt) |
| 上述测试加 `--all-features` | 146 通过，0 失败，3 默认忽略 | [native 测试日志](../reports/p1-guest-native-tests.txt) |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过 | [默认 Clippy](../reports/p1-guest-clippy.txt) |
| 上述 Clippy 加 `--all-features` | 通过 | [native Clippy](../reports/p1-guest-native-clippy.txt) |
| 默认与 native CLI 构建 | 均通过 | 两种构建的 CLI 集成测试均通过 |

两种测试配置的大部分用例相同，不能将其相加称为 292 项不同测试。native 配置包含两项实际 libvirt TEST driver 测试；默认配置验证未编译绑定时明确返回 skipped。

三个默认忽略的入口分别是 SQLite 崩溃子进程辅助入口、真实 QEMU COW 实验和真实 TCG Guest 启动。SQLite 入口由父测试调用；COW 实测证据保留在[上一阶段记录](p1-verification.md)；本轮明确执行 Guest 集成测试及 CLI 实验，结果如下。

## 真实 Linux Guest

```bash
CARGO_HOME=/tmp/elasticpc-next-cargo-home \
ELASTICPC_TEST_GUEST_TOOL_DIR=/tmp/elasticpc-next-runtime/root/usr/bin \
ELASTICPC_TEST_GUEST_FIRMWARE_DIR=/tmp/elasticpc-next-runtime/root/usr/share/qemu \
ELASTICPC_TEST_GUEST_KERNEL=/boot/vmlinuz-6.12-x86_64 \
cargo test -p elastic-lab --locked --offline real_qemu_tcg_guest_boot -- --ignored --nocapture
```

显式集成测试通过。CLI 完整调用也返回退出码 0，报告保存为 [p1-linux-guest-boot.json](../reports/p1-linux-guest-boot.json)：

- `status=passed`、`evidence_kind=real_qemu_tcg_guest_boot`，9 项检查全部通过。
- QEMU 11.1.2、TCG、256 MiB、1 vCPU；启动前可用内存满足 8704 MiB 的保守门槛。
- 串口包含本次 nonce 对应的 PID 1 ready、poweroff 标记，以及内核 `reboot: Power down`。
- QEMU 正常退出，无超时；临时文件和目录清理成功。
- 无磁盘、网络、共享目录或 Host 设备接入；`kvm_validated=false`。

内核文件 SHA-256 为 `3fcab855edde783347f55a9fc3e20909f870f8c476bf6e4e5709217e35006af6`。固件、生成的 PID 1/initramfs 散列和实际命令均保存在报告中。

## 真实 libvirt API、模拟域

native 检查使用临时运行库环境：

```bash
export CARGO_HOME=/tmp/elasticpc-next-cargo-home
export PKG_CONFIG_PATH=/tmp/elasticpc-next-runtime/root/usr/lib/pkgconfig
export LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib
cargo build -p elasticctl --locked --offline --features native-libvirt
./target/debug/elasticctl lab libvirt-test --json --redact
```

CLI 返回退出码 0，[p1-libvirt-test-driver.json](../reports/p1-libvirt-test-driver.json)记录 `status=passed`、9 项检查全部通过、`cleanup_complete=true`。实际调用 libvirt 12.8.0 与 `virt 0.4.3`，测试域创建、启动、实时 XML 观察、停止和删除完成，原有测试域清单保持一致，连接关闭成功。

此处 `backend_simulated=true`、`guest_boot_verified=false`；没有连接 Host QEMU 守护进程。未知 XML、块设备非 raw backing chain、ROM 文件、未知源及连接清理均有相应保守处理或回归测试。

为保持默认 CLI 不依赖临时 libvirt 库，最后恢复了默认构建；native 构建副本位于 `target/debug/elasticctl-native-libvirt`，需上述 `LD_LIBRARY_PATH` 才能使用。该目录是可重新生成的构建输出，不是安装产物。

## 主机证据与结论边界

[本轮 readiness](../reports/p1-guest-host-readiness.json)明确记录：KVM 已在内核注册，但本进程不见 `/dev/kvm`；本次 API 探测没有打开节点或执行 ioctl，因此 KVM API 仍未验证。T7 的 udev by-id 记录存在，但当前命名空间没有可解析的对应别名，执行身份仍未通过。

本轮成功证明隔离 Linux 的最小启动/关机链，以及 libvirt 绑定在测试驱动上的生命周期。尚未证明 KVM 加速、现有 Windows/Manjaro 登录、OVMF/vTPM、原盘隔离、Host 全局占用对账或持久控制器的生产后端接入。没有安装系统软件包、修改分区/EFI/驱动绑定或启停生产服务。

实现、依赖来源和下一阶段见 [实施说明](p1-guest-and-libvirt.md)。
