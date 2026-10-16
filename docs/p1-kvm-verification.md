# 隔离 KVM 主机验收

日期：2026-10-13。用户明确批准“批准，进行测试”，范围为 1 vCPU、256 MiB、无磁盘、无网络的最小 Linux KVM 实验。

## 实施内容

`guest-boot` 保留默认 TCG，增加显式 `--kvm`，通过同一内存门禁、输入复制、静态 PID 1、initramfs、串口证据与清理流程。KVM 参数只指定 `accel=kvm`，无 TCG 回退；无需 root，不安装系统包，不连接主机 libvirt。

固定运行参数包含 `-nodefaults`、`-nic none`、`-monitor none`、`-display none`、`-no-reboot`、QEMU seccomp sandbox；无 drive/blockdev/共享目录。Guest 最长运行 60 秒，当前 MemAvailable 须覆盖 8192 MiB Host 预留、256 MiB Guest 及 256 MiB QEMU 余量。

## 测试结果

[真实报告](../reports/p1-kvm-guest-boot.json)：`status=passed`、`evidence_kind=real_qemu_kvm_guest_boot`、`kvm_validated=true`，9 项检查全部通过。Guest 串口返回本次 nonce 的 PID 1 ready 和 poweroff 标记，内核确认 `reboot: Power down`，QEMU 退出码 0，未超时、无 stderr；实验临时目录清理成功，并另行核对目录已不存在。启动时 MemAvailable 为 18794 MiB。

QEMU 11.1.2 和固定固件及依赖只解包到 `/tmp/elasticpc-kvm-runtime`，每个包核对既有散列并使用系统 Arch keyring 验签。QEMU 私有副本设置相对 RPATH，原始和修改后散列都记录在 [依赖来源](../reports/p1-kvm-runtime-provenance.json)。这套临时工具缓存仍保留用于复现；已清理的是本次 Guest 的内核/固件副本、initramfs 和构建文件。

实际执行命令（已批准的沙箱外当前用户环境，无 sudo）：

```bash
./target/debug/elasticctl lab guest-boot --kvm \
  --kernel /boot/vmlinuz-6.12-x86_64 \
  --tool-dir /tmp/elasticpc-kvm-runtime/root/usr/bin \
  --firmware-dir /tmp/elasticpc-kvm-runtime/firmware \
  --json > reports/p1-kvm-guest-boot.json
```

相同命令增加 `--dry-run` 的 [报告](../reports/p1-kvm-guest-dry-run.json)确认依赖齐备，未创建临时文件或运行命令。

软件验收：`cargo test --workspace --locked --offline` 为 151 项通过、0 失败、3 项默认忽略，见 [日志](../reports/p1-kvm-workspace-tests.txt)。随后 `cargo test -p elastic-lab --locked --offline kvm_` 的 3 项针对性测试全部通过，包含补充的 root/内存不足拒绝检查。`cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` 均通过，见 [Clippy 日志](../reports/p1-kvm-clippy.txt)。本轮未修改 native libvirt 路径或重新启动 TCG Guest。

## 尚未解决

本次只证明最小 Linux kernel + initramfs 在 KVM 上启动/关机，不证明现有 Windows/Manjaro、桌面登录、UEFI/OVMF、TPM、GPU 直通或生产负载兼容性。原始 24 GiB Guest 加 8 GiB Host 的内存方案仍超过 27.21 GiB 实际 RAM；resume/swap 和服务数据迁移仍需核对。

## 下一阶段需要批准的主机变更

下一阶段先准备独立普通文件镜像的 QEMU/libvirt 生命周期及持久恢复实验、连接方式和清理计划。若需连接主机 libvirt socket 或启动守护进程，应提交具体影响与回退后另行授权。本次授权不包含原盘读写测试、EFI/NVRAM、驱动绑定、生产服务启停或重启。
