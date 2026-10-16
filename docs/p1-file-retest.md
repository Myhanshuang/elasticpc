# 文件镜像与恢复测试复验

日期：2026-10-13。用户要求继续测试，本轮运行现有文件镜像和恢复验收，不连接主机 libvirt 服务。

## 实施内容

恢复固定版本 QEMU 11.1.2 的 `qemu-img` / `qemu-io`，核对既有 SHA-256 和 Arch 签名，只解包到临时目录；没有安装系统软件。来源见 [依赖记录](../reports/p1-file-retest-runtime.json)。本轮没有修改执行逻辑。

## 测试结果

| 验收 | 实际结果 | 证据 |
|---|---|---|
| 普通文件 raw + qcow2 | 14 项检查通过，8 MiB 私有基底只读；覆盖层写入后全文件基底哈希一致；主动改变临时基底后拒绝旧覆盖层；临时文件清理成功 | [COW 报告](../reports/p1-file-retest-cow.json) |
| SQLite + mock | 持久意图、冲突拒绝、控制器重建、幂等重放和服务域隔离通过，清理完成 | [mock 恢复](../reports/p1-file-retest-mock-recovery.json) |
| SQLite + libvirt TEST | 保留 TEST 会话重建控制器，未知观察拒绝，停止 desktop 保留 service，清理完成 | [TEST 恢复](../reports/p1-file-retest-libvirt-recovery.json) |
| 全 feature workspace | 157 项通过，0 失败，3 项默认忽略；包含 SQLite 子进程崩溃父测试和 TEST 六处中断边界测试 | [测试日志](../reports/p1-file-retest-native-tests.txt) |

执行命令：

```bash
CARGO_HOME=/tmp/elasticpc-next-cargo-home \
PKG_CONFIG_PATH=/tmp/elasticpc-next-runtime/root/usr/lib/pkgconfig \
LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib \
cargo test --workspace --all-features --locked --offline

LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib \
./target/debug/elasticctl-native-libvirt lab file-cow \
  --tool-dir /tmp/elasticpc-kvm-runtime/root/usr/bin --json

LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib \
./target/debug/elasticctl-native-libvirt lab libvirt-recovery --json

./target/debug/elasticctl lab recovery --json
```

native CLI 副本在测试结束后保留，默认 CLI 恢复为不依赖临时 libvirt 库的构建。

## 尚未解决

真实文件块 I/O、之前的无盘 KVM 启动以及 TEST 控制器恢复是三组独立证据。目前尚未实现真实 Guest 挂载实验磁盘后的跨进程持久恢复，不能把三者合并宣称该流程已经通过。TEST 驱动本身模拟域状态，并不证明真实设备 I/O 已停止。

## 下一阶段需要批准的主机变更

先实现独立文件镜像 Guest 和真实进程观察/恢复的连接点及清理方案。若需要连接主机 libvirt socket、启动守护进程或调整系统配置，提供具体操作和回退后另行申请。当前复验未写真实块设备、修改 EFI 或驱动，也未启停生产服务。
