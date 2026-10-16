# 本轮最终顺序与并行运行（v9）

运行 `epc-test`：**Manjaro 原盘直写 → Windows RAM 覆盖层 + NVIDIA GPU 直通**。Windows 不再先测无 GPU 版本。接好 NVIDIA 对应的外接显示器，并保留 AMD Host 屏幕或远程终端。

脚本按顺序验收；运行程序已取消单 Guest 限制。手动并行启动：

```sh
epc start linux --mode direct
epc start windows --mode trial --gpu
```

Windows 8 GiB、Manjaro 6 GiB，Windows 覆盖层 6 GiB，Host 预留 6 GiB；总量或实际可用内存不足仍拒绝启动。并行能力和 GPU 实机稳定性仍待验证。

`linux` 是 `manjaro` 的别名。查询或启动会整理上次启动遗留的运行状态，也可执行 `epc clear-stale linux` / `epc clear-stale windows`。只整理已确认没有活动服务、锁、QEMU 或挂载的旧启动状态，不删除日志、旧归档，不冒充保存完成。

本节覆盖后文历史版本的顺序和单 Guest 限制。

---

# 紧急停止且不保存（v9）

Windows 使用 RAM 覆盖层时：

```sh
epc stop windows --force --discard
epc status windows
```

当前试运行中的全部修改都会丢弃；原盘和其他历史归档不删除。即使已开始保存，也会请求取消；正在执行的 USB 刷新可能仍要等待。确认 `status: discarded`、`archive: discarded` 且服务退出后才能关闭 Host。无需关闭整台物理机来终止 Guest。

Manjaro 原盘直写没有覆盖层，不需要保存步骤。卡死时可用：

```sh
epc stop manjaro --force
```

已写入原盘的修改无法撤销；强停不算正常关机验收。直写模式拒绝 `--discard`。

`epc-test` 在 Windows 停止前和保存等待中提供输入 `DISCARD` 的选项，默认仍保存。丢弃会单独记录，不计为正常关机通过。本轮 USB 只增量更新运行程序、说明和已授权的 Manjaro 直写许可，不覆盖最近测试日志，不做整盘备份。

---

# 下一轮：Manjaro 原盘直写 / Windows 内存覆盖层

2026-10-15 用户明确授权此组合。新版镜像位于 `target/host-mixed-v8/`；是否已写入 U 盘以本轮交付说明为准。

启动外部 Host 后执行：

```sh
epc-test
```

脚本逐个启动 Windows（6 GiB RAM 覆盖层）和 Manjaro（原盘直写），本轮跳过 GPU。Manjaro 原盘授权绑定已试启动磁盘的序列号及头部哈希；不合并此前覆盖层，不把此次授权冒充为干净关机或原生回退验收。启动时仍检查所有挂载命名空间、swap、打开设备的进程、磁盘身份和 ext4 clean 状态。

Manjaro 的文件及系统配置修改直接落到原盘，正常关机无需覆盖层归档。若被强制停止或异常退出，脚本不会认定直写测试通过。Windows 必须等待 `archive:saved`；复制完成后可能仍在等待 USB 落盘或校验。不要长按 Host 电源代替等待。

脚本完成且 `safe_to_poweroff:true` 后执行 `sync`、`poweroff`，拔掉 U 盘，原生启动 Manjaro，验证桌面、测试文件、网络和正常关机，并反馈结果。原生回退必须由这次实机验证确认。

手工启动时必须显式指定模式：

```sh
epc start windows --mode trial
epc start manjaro --mode direct
```

不要同时运行。查询密码用 `epc password windows` 或 `epc password manjaro`。正常关机用 `epc stop <系统名>`；只有确实无响应时才显式加 `--force`，Manjaro 强停后应先检查文件系统，不计作通过。

以下为历史试运行说明，模式与本节冲突时以本节为准。

---

# Kingston Host：下一次集中验收

本轮采用：首次完整覆盖层试运行并保留；验证必要驱动后增量安装到原系统、保留原驱动；再显式切换原盘读写。驱动安装包另外保存，不做“驱动专用块覆盖层”。

## V6：登录后运行引导测试

登录 USB Host 的 root 后执行 `epc-test`。脚本依次进行网络基线、Windows 无 GPU、Manjaro 和可选 Windows 显卡直通；只用 trial，同一时间只开一套 Guest。提示使用短英文，兼容物理控制台。

观察阶段按 `P` 记录桌面/应用/网络通过，`F` 记录失败，`D` 保存诊断，`C` 再显示控制台，`W` 找回密码，`N` 采样 Guest 运行时的网络，`R` 请求硬重启，`Q` 结束流程。强制操作需明确输入确认词。退出后脚本不会自动关 Host；归档安全检查通过后才提示手动关机。

报告在 `/root/epc-tests/日期时间/summary.json`，旁边是本轮诊断。保存失败、旧会话误用、锁/挂载残留或设备清理异常会阻止继续。QMP 就绪与人工桌面验收分别记录；跳过的 GPU/网络项目不算已测。当前不做原盘模式及双 Guest 并行验收。

新增脚本部署以 `reports/kingston-host-tests-v6.json` 为准；按用户要求，写入完成状态为 `written-unverified`，不进行整盘写后回读校验。以下为手工备用步骤。

## V5：本轮重测方式

试运行改用 3 GiB、不换出到磁盘的内存覆盖层，同一时间只运行一套 Guest。接近容量上限时暂停。正常停止或强制停止 Guest 后，Host 会保存覆盖层到 U 盘并核对哈希；只有状态的 `archive` 为 `saved` 后才可关闭 Host。`saving` 表示仍在落盘；`save-failed` 时内存副本保留，用 `epc save windows`（或 manjaro）重试，不能关闭 Host。整机强制断电会丢失未落盘修改。

本轮先单独测试 Windows，不加 GPU 参数；确认能登录后正常关机，等待保存完成，再测试 Manjaro。

```sh
epc start windows --mode trial
epc status windows
epc console windows
epc password windows
```

重复 console 会再次显示地址和密码，不会因为控制台服务已存在而退出。密码是当前会话的密码。

```sh
epc stop windows                 # 请求正常关机
epc stop windows --force         # 强制断电，未保存的应用数据可能丢失
epc restart windows --force      # 硬件重置，同一会话和覆盖层
epc diagnose windows             # 保存磁盘统计及当前屏幕，供排查转圈/蓝屏
epc status windows               # 确认 archive: saved 后才能 poweroff
```

以上 windows 可换成 manjaro。强制停止后的状态是 forced-stopped，不能作为直接使用原盘的验收结果。

Windows 虚拟 CPU 暂不暴露嵌套虚拟化，原系统 Hyper-V / VBS 配置和驱动不删除；本轮不验收 Guest 内的 WSL2/Hyper-V。HYPERVISOR_ERROR 是否消失仍需实机确认。

Manjaro 改用 virtio-blk 磁盘路径；仅虚拟启动禁用原磁盘 swap。试运行时临时屏蔽 Docker/containerd/k3s、MySQL/PostgreSQL 自启动，以减少后台写入；不修改原盘服务配置。原生启动不受这些内核参数影响。

旧的全部试运行覆盖层保留；仅回收其全零块，保存前后逐文件哈希必须一致。旧覆盖层不自动续用或合并到已变化的原盘。

本轮按用户要求跳过 U 盘整盘回读校验。部署报告：`reports/kingston-host-repair-v5.json`，完成状态为 `written-unverified`（已写入、未校验）。镜像内附带说明可能仍写旧状态 `passed`，以此处和实际部署报告为准。以下旧版容量/同时运行流程不适用于本轮内存试运行。

## 2026-10-15 首次双系统故障修复

已定位并修复：真实磁盘误用 QEMU 普通文件驱动；NTFS 工具在缺失休眠文件时返回 0 导致 Windows 被误拒绝。启动前错误会持久记录，`epc status` 可显示原因。新镜像需以 `reports/kingston-host-repair.json` 的 `status: passed` 确认部署完整。此前截图已显示一次 Tailscale 直连成功，但两套系统尚未完成实机启动验收。

请在系统中正常关机，长按电源键不能代替干净关机。若命令显示失败，保留 status 输出；详细服务日志可用 `journalctl -u elasticpc-windows --no-pager -n 40` 或将 windows 换成 manjaro。

## 已验证与待验证

项目普通文件测试已通过：新密码登录、USB 模拟启动、NVMe 模拟启动、覆盖层写入后底盘完整哈希不变。原 Windows / Manjaro 启动、物理显卡直通和新 Host 的 Tailscale 直连尚待以下实机验收。U 盘部署结果以 `reports/kingston-host-deployment.json` 的 `status: passed` 为准。

## 启动和联网

1. 从一次性 UEFI 启动菜单选择 Kingston；root 密码使用你修改的 local-login.txt 内容。
2. 执行 `epc-diagnostics`，关机前将 `/run/elasticpc-diagnostics` 复制到 `/root/diagnostics`，保留启动耗时和内存压力证据。
3. 执行 `nmtui` 联网，再执行 `tailscale up --hostname=elasticpc-host --accept-routes=false`。在手机上打开输出的登录链接。此 Host 使用独立设备身份，不复制原系统的登录状态。
4. 执行 `epc-network-check 对端的Tailscale地址`，对端必须在线。报告的 `direct_verified` 为 true 才代表此次确认直连；能 ping 通不等于直连。不能直连时保留两端 netcheck 输出。

## Windows 与显卡

1. 执行 `epc start windows --mode trial`，随后 `epc status windows` 和 `epc console windows`。
2. 用已登录同一 Tailscale 网络的手机或另一台设备打开输出的网页，输入输出的 VNC 密码。
3. 检查登录、应用、网络。出现 BitLocker 恢复时先停止并保留提示，不能格式化或自动修复原盘。
4. 在 Windows 内正常关机，确认 `epc status windows` 显示 stopped。
5. 执行 `epc start windows --mode trial --gpu`。内屏保留给 AMD / Host，外接显示器接 NVIDIA 对应输出；网页控制台可能是另一块虚拟屏幕。检查 NVIDIA 设备、驱动和日常图形负载，然后正常关机。

## Manjaro

执行 `epc start manjaro --mode trial`、`epc status manjaro`、`epc console manjaro`，检查桌面、应用、网络并正常关机。Host 不能挂载原系统分区。当前使用已导出的内核和 initramfs，关闭休眠恢复。

## 保留试运行、准备原盘模式

每次 start 都创建新的 3 GiB 覆盖层，旧层全部保留，不自动合并或删除，也不自动续用。空间即将耗尽时暂停虚拟机；不要在试运行中做大型系统升级。原系统重新启动或发生写入后，旧层不能直接接到变化后的原盘继续使用。

将验证过的驱动安装包、版本、来源和 SHA256 另存到 `/var/lib/elasticpc/driver-packages`，并保存一份到可靠存储。若确有新增驱动需求，在原系统中单独增量安装，保留原驱动；不整块复制注册表或 DriverStore。安装后应重新进行相关启动验证。

确认试运行和原系统驱动都准备好后，分别执行：

```sh
epc accept-trial windows --confirm-boot-and-drivers --confirm-native-drivers-ready
epc accept-trial manjaro --confirm-boot-and-drivers --confirm-native-drivers-ready
epc start windows --mode direct --gpu
epc start manjaro --mode direct
```

direct 模式直接写原系统磁盘，日常文件和设置随之保存到原盘；不会自动提交试运行内容。先分别验证，再同时启动。默认 Windows 12 GiB、Manjaro 8 GiB，Host 至少保留 6 GiB；资源不足时拒绝启动。

## 原生回退与异常记录

先正常关闭两套 Guest、确认 stopped，再关闭 Host，拔掉 Kingston，正常启动原 Windows / Manjaro。原盘启动器和原驱动不会被本次镜像部署替换。

命令拒绝或启动失败时，记录提示及对应 session 的 qemu.log，不绕过保护检查。不要在运行中拔盘。覆盖层留在 U 盘上，因此首次试运行性能仍可能受 USB 写入速度影响；原盘模式的 Guest 主数据直接使用原 SSD。
