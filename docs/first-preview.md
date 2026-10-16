# 第一个可体验版本：本地文件 Trial 控制台

这是可登录、可保存文件并重新启动的 Debian 串口体验版，验证 ElasticPC 的操作流程。它不是 Windows/Manjaro 图形双桌面，也不包含 GPU 直通、网络或真实系统导入。

## 开始体验

本次交付已准备好默认体验目录，可跳过 init，直接从 start 开始。新检出或新档案才需要 init。

在项目目录打开终端：

```sh
./elasticpc init       # 每个体验目录只需一次；已有目录会拒绝覆盖
./elasticpc start
./elasticpc console
```

稍等 Guest 启动，看到 `elasticpc-preview:~#` 后，可输入：

```sh
echo '我的第一次体验' > /root/experience-note
cat /root/experience-note
poweroff
```

随后在主机终端再次运行 `./elasticpc start` 和 `./elasticpc console`，文件仍应存在。控制台中按 **Ctrl-]** 仅断开连接，Guest 保持运行。不要把 Guest 控制台当作主机终端：自动登录的 root 属于隔离 Guest。

```sh
./elasticpc status       # Running / Stopped / Unknown
./elasticpc stop         # ACPI 正常关机，最多等待约 30 秒
./elasticpc recover      # 从新观测恢复控制器状态
./elasticpc stop --force # 正常关机失败时显式强制退出，可能丢失未保存数据
```

`Running` 表示 QEMU 进程在运行，不代表操作系统已经完成登录。控制连接不明或锁仍被持有时，不把它推断成 Stopped。重复 start 不创建第二个 Guest，重复 stop 保持停止。

## 数据位置与资源

- `.elasticpc/preview/`：默认体验档案，包括只读底盘副本、可写覆盖层、独立 OVMF 变量与 SQLite 控制器日志。
- `.elasticpc/images/`：构建完成的只读体验镜像基线。
- `.elasticpc/runtime/` 与 `.elasticpc/inputs/`：从已验证的临时依赖复制的本地运行环境，不安装主机软件。
- `--directory /绝对路径` 可选择另一个已准备好的短路径私有档案。init 会创建新档案，其他操作要求其已存在。

每次启动使用 1 vCPU / 1 GiB RAM，并要求主机至少 9472 MiB MemAvailable。KVM 必须对当前普通用户可用；程序不提权、不加载模块。没有网卡、SSH 登录或真实磁盘透传，界面是本地串口。请勿将这个自动登录镜像部署到共享或联网环境。

不要删除正在运行的档案、锁文件或手动修改底盘。停止后可以保留整个档案；不要仅复制 overlay 而丢失它指向的底盘。路径绑定的实验档案目前不支持直接移动，底盘内容或身份变化会阻止再次启动。这里没有自动提交覆盖层到原盘的入口。

`.elasticpc/` 与 target 编译目录分开，普通 cargo clean 不删除体验数据。它仍位于同一块 SSD 上，**不是独立备份**。

## 实现与故障恢复

统一入口调用现有 Rust `DurableController` 和 SQLite 日志，通过限定普通文件的预览适配器控制独立监督进程。物理设备租约被拒绝；文件排他性由跨进程 flock 与 QEMU 的文件锁实现。控制器进程中断后，可按新观测恢复；监督进程消失时，Linux parent-death signal 结束 QEMU，继承的锁在 Guest 退出前继续保护文件。

停止请求只有在 Guest 退出、底盘重新验证和生命周期锁释放后才成功响应。未知状态保留阻塞，不能因为一个 PID 文件缺失就授权重启。持久日志不是任意宿主掉电情况下的完整数据恢复保障。

## 构建入口

已有本地已验证依赖时：

```sh
cargo build -p elastic-controller --example experience_controller --locked --offline
python3 scripts/build_experience.py --output target/experience-image
python3 scripts/package_experience.py
```

构建镜像约需数分钟，输出目录必须不存在。依赖准备沿用 P2 文档，构建仅在明确授权的文件磁盘 KVM Guest 中执行。当前镜像和二进制是本机工程体验产物，没有对外发布。

## 验收

2026-10-14 完成验收，详见 [机器报告](../reports/experience-integration.json)。16 项真实操作检查全部通过：CLI 登录与断开、重复启停、正常关机、文件跨重启、控制器在 AfterStart/AfterStop 被 SIGKILL 后恢复、监督进程消失时 Guest 退出与再次启动，以及旧底盘拒绝。

软件检查通过 156 项 Rust 测试、7 项体验脚本测试、10 项 P3、9 项 P2、2 项原有私有实例测试，以及 Clippy（warnings 视为错误）与 fmt。验收后源码摘要与报告一致。默认体验目录已初始化、指纹校验通过，交付时为 Stopped；测试 Guest 已回收。Windows 检测、Manjaro EFI Boot Bridge、真实系统导入、GPU 和显示切换仍待后续阶段。
