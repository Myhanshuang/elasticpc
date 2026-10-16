# P3 文件镜像 Trial 增量

当前推进 P3 的普通文件镜像部分。用户目前没有独立备份，基本不用休眠；后者尚不构成删除 swap 的决定。真实 Windows/Manjaro 导入、T7 设备访问、真实 EFI 修改与 SSD 分区操作均未执行。

## 已实现的文件档案

`scripts/trial_image.py` 为显式指定的 raw 或独立 qcow2 文件准备专用 Trial 目录。输入必须是当前用户拥有、非软链接、单硬链接、没有写权限位的普通文件；固件变量输入同样校验。文件经打开的描述符复制到新目录，不按原路径重新打开复制。输入变化时拒绝准备。

每个 Trial 保存独立 `base.img`、`overlay.qcow2`、`vars.fd`、`manifest.json` 和 `session.lock`。底盘私有复制后设为只读，覆盖层明确指定 backing format；拒绝已有 backing chain、加密或外部 data-file。目录必须为当前用户私有，新建失败只清理本次创建目录，不覆盖已有目录。

指纹包含底盘整个文件 SHA256、字节数、设备/inode 与纳秒时间戳。启动前重新计算并核对，任何变化均拒绝继续使用旧覆盖层；这比只比较 GPT 或容量更保守。覆盖层也核对 backing 路径、格式与虚拟容量。固件变量由 Guest 独立写入，正常变化不会被误当底盘失效。

`session.lock` 在同一 Trial 的检查与 Guest 整个生命周期内持有排他锁，进程结束后释放。该锁只约束遵守协议的程序；同一用户主动改权限、替换文件或绕过实验入口仍可能破坏隔离。运行中的源盘并发写入防护、全局租约与生产控制器尚未接入，不能据此授权真实块设备。

## 使用

依赖 P1 已验证的临时 QEMU 工具与 P2 固件/seed 工具。准备和验证本身不启动 VM，不要求 KVM，也不安装主机软件。

```sh
python3 scripts/trial_image.py prepare --directory target/my-trial \
  --source target/p2-host-release/elasticpc-host.qcow2 \
  --firmware target/p2-host-release/elasticpc-host-vars.fd --format qcow2
python3 scripts/trial_image.py verify --directory target/my-trial
make p3-check
make p3-dry-run
```

`--dry-run` 不读取源镜像、不创建目录或启动子进程。prepare 不应使用 `/dev/...`、活动系统盘或有外部 backing 的镜像。当前实验完整复制普通文件镜像，尚未实现真实 1 TB 底盘免复制映射；P3 对现有系统的正式交付仍待完成。

## KVM 实测范围

显式运行 `make p3-lab P3_OUTPUT=target/p3-trial-run`，从已经通过 P2 的 Debian Host 镜像创建私有 Trial。启动单台 1 vCPU / 1 GiB Guest，保留至少 8 GiB 主机内存与额外开销，无网卡、无真实磁盘、无主机 libvirt 连接。

验证顺序为正常启动关机、Guest 就绪后 SIGKILL、再次启动关机、覆盖层一致性检查，以及对自建底盘副本进行同尺寸字节修改后的拒绝验证。每次启动前后重新校验完整底盘。等待串口使用事件驱动读取，超时或异常必定杀死并回收本次 Guest。

最后的破坏性指纹测试只修改实验目录中的副本，会故意留下不可恢复的旧 Trial。不要把这个目录作为可继续工作的镜像；需要继续实验时使用 prepare 新建另一个目录。P2 交付基线不会修改。

SIGKILL 覆盖的是 QEMU 进程退出与磁盘覆盖层再次启动，不代表主机掉电、不保证应用事务完整，也不是内存快照恢复。此增量与 P1 的持久控制器崩溃实验分别记录。

## 仍待完成的 P3 内容

- Windows OS/EFI/BCD 安装关系、BitLocker/vTPM 和启动总线兼容性验证。
- Manjaro 分区级只读底层、独立 EFI Boot Bridge、内核更新跟踪、Guest swap/resume 配置。
- 全局占用重核验、与持久控制器租约及恢复日志对接。
- 有备份与回退条件后，真实 T7 与 Manjaro 均启动至登录界面，退出后原盘指纹不变。

因此本轮只能标记“P3 文件镜像增量通过”或“进行中”，不能标记 P3 整体完成。

## 2026-10-14 本轮结果

**P3 文件镜像增量通过，P3 整体仍在进行中。** [机器报告](../reports/p3-file-trial.json)记录三次真实 KVM Guest 运行：正常启动退出码 0、注入 SIGKILL 退出码 -9、同一覆盖层再次启动退出码 0。三次运行前后底盘指纹均一致；覆盖层 qemu-img check 通过。随后只改私有底盘副本中的一个字节，验证旧 Trial 被明确拒绝。

P2 交付镜像和固件变量重新计算的摘要与原交付报告一致。测试用旧 Trial 故意保持失效状态，不供继续启动。原始串口日志归档于 `reports/p3-trial-logs/`。

[十项 P3 测试](../reports/p3-file-tests.txt)全部通过，其中两项使用真实 QEMU 工具验证 raw/COW 写入隔离与 backing chain 拒绝；[九项 P2 回归](../reports/p3-p2-regression.txt)和[两项私有实例脚本回归](../reports/p3-private-regression.txt)通过。本轮未更改 Rust 模块，未重复此前已通过的 Rust 全量测试。

实测后另加了打开文件前的类型拒绝，以及 Trial 路径中的 QEMU 参数分隔符拒绝；十项文件测试覆盖这些提前拒绝分支。报告分别保留 KVM 实测源码摘要和这一加固后的工作区摘要。

## 后续已完成的体验版接入

普通文件 Trial 已通过独立预览适配器接入既有 Rust/SQLite 持久控制器，提供统一命令、串口登录和异常恢复，见 [体验版](first-preview.md)。本文件前述“控制器接入待做”现已由此文件范围内的适配器覆盖；真实设备全局占用、物理租约和生产系统导入仍未开放。
