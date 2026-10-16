# P2 隔离 Host 构建与验收

安装边界、两套布局与物理审批门禁见 [安装方案](p2-host-installation-plan.md)。此流程只使用项目输出目录、`/tmp` 依赖和 `/dev/kvm`；不连接主机 libvirt，不接入真实磁盘或网络，不写主机 EFI、不重启主机。

## 复现

需要普通用户可访问 KVM、至少 9472 MiB MemAvailable、足够的文件存储空间（建议 15 GiB），以及 Python 3、curl、gpg/gpgv、ar/tar、mkfs.vfat、strip。先按 P1 文档准备已验签的 `/tmp/elasticpc-integration-runtime`。依赖下载只在主机临时目录进行，Guest 始终无网络。

```sh
cargo build -p elasticctl --locked --offline
make p2-inputs
make p2-dry-run
make p2-host P2_OUTPUT=target/p2-host
```

构建磁盘的逻辑容量为 12 GiB，物理布局规划的 Host 根分区为 20 GiB；真实部署需另行验证镜像展开和扩容步骤。

输出目录必须不存在，避免覆盖任何已有镜像；失败时保留日志与报告用于排查，不自动重试。离线包准备被中断时可显式执行 `python3 scripts/prepare_p2_debs.py --resume`，会重验已有包的哈希。首次运行 `prepare_p2_inputs.py` 要求目标临时目录不存在。

构建使用固定 Debian 13 genericcloud `20261001-2618` 镜像。包下载记录具体版本与摘要；发行版仓库会变化，长期复现需要保存报告和下载缓存，不宣称只靠脚本即可永久逐位复现。

## 验收顺序

1. OVMF + KVM 启动官方基础镜像，从只读 FAT seed 离线安装 Host，保留已有较新安全更新；APT 检查实际依赖。
2. 导出独立 qcow2 和配套 OVMF vars，在全新验证 overlay 中正常启动，验证 doctor JSON、QEMU 命令和系统 libvirt 连接。
3. 通过 GRUB 的一次性选择进入独立 recovery target，核对实际内核参数与恢复诊断标记。
4. 卸载本项目的 EFI 目录、唯一入口、服务和可执行文件，逐文件核对原始 EFI 哈希。
5. 再启动同一个验证 overlay，确认 Debian 原始引导仍工作、ElasticPC 文件与入口已删除。测试用一次性验证服务随后自行删除并关机。
6. 对交付镜像及测试 overlay 执行 qemu-img check，记录镜像与源码摘要。

单次启动最多 600 秒；超时即终止并回收该 Guest，实验失败。正常关机及专用串口标记同时成立才通过。报告明确 `native_windows_manjaro_boot_verified=false`，不会将上述 Debian 回退冒充原生双系统验收。

构建镜像的正常入口是系统服务；恢复入口仅提供只读串口诊断。尚无 Host 图形登录、网络控制面、生产虚拟机启动 API 或真实 SSD 安装器。BIOS/Secure Boot 兼容性和操作员登录方案仍需真实部署前验证。

## 软件回归

`python3 -m unittest discover -s tests/p2 -v` 验证 dry-run 不落盘、内存门禁、超时杀进程并等待回收、缺少成功标记和失败标记优先规则。EFI 解析回归测试另覆盖名称与设备路径分隔、同名入口冲突、错误路径和缺失入口，避免误删其他系统入口。实测结果见下文。

## 2026-10-14 验收结果

**隔离交付通过；真实部署门禁尚未通过。** [完整机器报告](../reports/p2-host-integration.json)记录六阶段均正常退出且出现专用成功标记：构建、正常启动、设置恢复入口、实际恢复启动、卸载、卸载后 Debian 回退。Host 镜像与验证 overlay 的 qemu-img check 均通过。当前工作区的构建器及 Host 源码摘要与本轮实测一致。

- 交付镜像：`target/p2-host-release/elasticpc-host.qcow2`，逻辑 12 GiB，文件约 1.07 GiB，无 backing file。
- 配套固件变量：`target/p2-host-release/elasticpc-host-vars.fd`。
- 配套报告：`target/p2-host-release/report.json`。
- 镜像 SHA256：`b91e3e04bd70ef09492dff3e69923e06a22ddf0b87aad379b80734d8c367b042`。
- 固件变量 SHA256：`7d223fec1d033178393076994bed97b19781205d7b72698274d262caf271d85b`。
- 六次启动前 MemAvailable 均超过 18 GiB；每次只有一个 1 GiB Guest。
- [九项 P2 脚本测试](../reports/p2-harness-tests.txt)、[两项原有实验回归](../reports/p2-private-harness-regression.txt)通过，Python 与 shell 语法检查通过。本轮没有修改 Rust 模块，未重复运行此前已经通过的 Rust 全量回归。
- 原始串口日志保存在 `reports/p2-host-logs/`；基础镜像仍匹配 [原始 SHA512](../reports/p2-base-integrity.json)。

交付镜像及变量文件已设只读。后续实验应新建 qcow2 overlay 并复制一份变量文件再启动，避免修改已验收基线；无需再次安装主机软件。此目录在 target 下，执行构建清理前应另存需要长期保留的镜像。

初始无网卡的官方 cloud 镜像会等待 network-online 约 120 秒；构建后已禁用该等待服务，后续启动不依赖网络。串口的无视频模式提示来自无显示设备配置，不影响本次串口验收。所有测试均无真实设备透传。

独立备份位置/容量与休眠需求尚未确认，真实分区、主机 EFI、BIOS、驱动和主机启动顺序没有改变。`native_windows_manjaro_boot_verified` 保持 false；后续真实部署必须遵守 [安装方案](p2-host-installation-plan.md)，不能把本报告的 passed 解读为两套原生系统已经验证。
