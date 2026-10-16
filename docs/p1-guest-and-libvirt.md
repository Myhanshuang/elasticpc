# 隔离 Linux 启动与 libvirt 适配器

日期：2026-10-13。这一阶段将普通文件实验推进到 Linux 内核和用户态启动，并接入真实 libvirt API 的测试驱动。Windows/Manjaro 现有安装的导入、桌面登录、原生引导和生产设备控制仍需后续验证。

默认与 native 两种配置各 146 项测试通过，格式检查与严格 Clippy 通过。真实 Linux 启动和 libvirt 测试驱动实验各 9 项检查通过，详见 [验收记录](p1-guest-verification.md)。

## 实施方式

`elastic-lab::guest` 创建私有临时目录，将调用方指定的 Linux 内核和已验证固件复制进去；使用项目内固定汇编源码构建无 libc 的静态 PID 1，生成仅含控制台节点与 `/init` 的 initramfs。控制台节点只编码在归档中，不在 Host 创建设备。

QEMU 固定使用 TCG、1 vCPU 和 256 MiB Guest RAM，禁用网络、显示窗口、监视器、默认设备及用户配置，不接入磁盘、共享目录或宿主设备。启动前检查当前可用内存，保留 8 GiB Host 预算及 256 MiB QEMU 余量。启动期限为 60 秒；超时和输出截断不会被判定为通过。

成功证据要求本次生成的标记由 Guest PID 1 从串口输出，随后输出关机请求，内核确认 power down，QEMU 正常退出。仅出现 QEMU 版本、启动命令或进程存在不代表启动成功。所有临时内核、固件、构建文件与 initramfs 在进程结束后清理。

这个选择使当前不可访问 KVM 的环境仍能验证 Linux 启动链；报告始终标记 `acceleration=tcg` 与 `kvm_validated=false`。TCG 的结果不能推断 KVM 性能或直通能力。[QEMU 启动参数](https://www.qemu.org/docs/master/system/invocation.html) · [Linux initramfs 格式](https://docs.kernel.org/driver-api/early-userspace/buffer-format.html)

## libvirt 适配器

新增 `elastic-virt-libvirt`，通过官方 `virt 0.4.3` 绑定调用 libvirt。默认构建只包含保守的 XML 观察解析；`native-libvirt` feature 启用实际绑定，构建时需要 libvirt 开发文件，运行时需要对应共享库。现有 SQLite/mock 控制器保持独立，尚未将这个测试适配器接成生产执行器。

连接入口只允许精确的 `test:///default`。空 URI、`qemu:///system`、`qemu:///session`、远程 URI 和自定义测试文件 URI 在连接前被拒绝；CLI 没有任意 URI 选项。实验只管理自身唯一命名的域，依次定义、启动、读取状态/XML、停止、删除定义，并验证清理结果。

libvirt 的 test driver 在进程内模拟域，适合验证 API 和生命周期，不启动 QEMU Guest，也不连接主机虚拟化守护进程。其报告单独标记 `backend_simulated=true`、`guest_boot_verified=false`，与上面的真实 Linux 启动证据分开。[libvirt test driver](https://libvirt.org/drvtest.html) · [Rust 绑定](https://gitlab.com/libvirt/libvirt-rust)

XML 观察提取域 UUID、状态、存储引用和 hostdev；不能解析的资源、未知状态或缺少运行配置时保留未知。顺序读取域列表/XML 不构成全局原子快照，测试驱动结果也不能释放真实设备租约。完整 Host 挂载、进程句柄、其他命名空间和实际 QEMU I/O 对账仍未实现。[Domain XML](https://libvirt.org/formatdomain.html)

## 命令与复现

默认构建不要求系统安装 libvirt：

```bash
cargo build --workspace --locked
./target/debug/elasticctl readiness --check-kvm-api --json --redact
./target/debug/elasticctl --dry-run lab guest-boot \
  --kernel /absolute/path/to/vmlinuz --json
./target/debug/elasticctl lab guest-boot \
  --kernel /absolute/path/to/vmlinuz \
  --tool-dir /absolute/verified/bin \
  --firmware-dir /absolute/verified/firmware --json
./target/debug/elasticctl --dry-run lab libvirt-test --json
```

Guest 工具目录仅选择固定名称 `qemu-system-x86_64`；固件也使用固定名称。内核源须为普通、单链接、不可由组/其他用户写入的绝对规范路径文件。来源在复制前后重新核对；原文件只读。调用方不能追加 QEMU 参数。`--dry-run` 只检查依赖元数据，不构建 initramfs、执行工具或连接 libvirt。两个实验均拒绝 `--fixture`，避免将 fixture 当作实测结果。

启用 native adapter 的普通系统构建与调用：

```bash
cargo build -p elasticctl --locked --features native-libvirt
./target/debug/elasticctl lab libvirt-test --json
```

未启用 native feature 时，此实验返回 `skipped`、退出码 3。实验通过为 0；失败或拒绝为 1。缺少 QEMU/内核/固件时也会明确返回未运行，不能算作成功。

## KVM 与设备身份

默认 `probe` / `readiness` 只读设备元数据，不打开 `/dev/kvm`。显式 `--check-kvm-api` 才在核对设备类型、设备号和内核注册后尝试只读打开并调用 `KVM_GET_API_VERSION`，不创建 VM、不加载模块、不自动提权；`--dry-run` 不执行该探测。fixture 模式始终离线，即使输入中记录 API 12，也不会把本次调用标记为已实测。[Linux KVM API](https://docs.kernel.org/virt/kvm/api.html#kvm-get-api-version)

本轮调查发现内核 sysfs 注册了 KVM，而当前进程的设备命名空间缺少 `/dev/kvm`。udev 数据库记录了 T7 的 USB by-id 别名，但当前命名空间没有 `/dev/disk` 可供解析。因此新增 `kernel_registered` 与 `udev_recorded_by_id` 分别记录这些证据；未解析的 udev 记录不会升级为可执行设备身份，也不会授权打开原盘。

## 临时运行环境

本轮从 Arch 官方软件包获取 QEMU 11.1.2、SeaBIOS、libvirt 12.8.0 及缺少的共享库，使用已有系统 Arch 密钥验签，只将所需文件解包到 `/tmp/elasticpc-next-runtime/root`，没有执行安装脚本或修改系统包。QEMU 的私有副本设置相对 RPATH 以定位同目录运行库，原包和变更后二进制分别记录散列。来源与验签记录见 [运行环境记录](../reports/p1-guest-runtime-provenance.json)。

此临时环境的 native 构建需要：

```bash
export CARGO_HOME=/tmp/elasticpc-next-cargo-home
export PKG_CONFIG_PATH=/tmp/elasticpc-next-runtime/root/usr/lib/pkgconfig
export LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib
cargo build -p elasticctl --locked --offline --features native-libvirt
```

这些设置仅适用于本次解包环境；安装了正常开发依赖的机器不需要这些临时路径。不要在工具消失后将本次报告当作当前依赖检查。

## 后续工作

后续先在正常 Host 环境核实 KVM API/Guest 加速、T7 别名解析和完整占用，再将 libvirt 观察与持久租约对账接起来。需同时明确 resume/swap、服务数据迁移和适合实际可用 RAM 的 Guest 配额。现有原盘 Trial、OVMF/vTPM、Windows/Manjaro 登录、VFIO 与显示切换不由本次无盘实验推导通过。
