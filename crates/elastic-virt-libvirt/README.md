# elastic-virt-libvirt

本 crate 隔离官方 `virt` Rust 绑定。当前实现只接受精确 URI `test:///default`，并在连接后核对 URI 与 `TEST` 驱动；空 URI、Host system/session、远程 URI、自定义 test 文件和 URI 参数都在调用 libvirt 之前拒绝。没有连接 Host QEMU 的接口。

## 接口与证据边界

- `parse_domain_xml(uuid, active, state, xml)`：纯 XML 解析，不打开 XML 中的路径。收集 file/block 磁盘、完整 backing chain、简单固件路径、PCI hostdev 与设备 ROM 引用。无法识别的存储类型、额外来源、扩展元素、未知状态、UUID 不匹配或缺失 live XML 均保留为 unknown。
- `read_test_driver_inventory(uri)`：只读连接 test driver，以 `list_all_domains(0)` 枚举活跃与非活跃域，读取 UUID、活动状态、状态及 `get_xml_desc(0)`。活动域使用当前 XML；非活跃域明确标记 inactive。前后状态变化或任一读取失败都不能变成完整观察。
- `run_test_driver_lab()`：内部生成唯一名字，检查名字尚不存在，定义无磁盘、无 hostdev、无网络的测试域；验证 inactive → running → shutoff；只对本次返回的 UUID/name/handle 清理。最终库存与初始库存比较，默认测试域保持不变，连接关闭也属于清理验收。

`test:///default` 是真实 libvirt C API 的进程内模拟驱动。实验报告使用 `evidence_kind=real_libvirt_test_driver`、`backend_simulated=true` 和 `guest_boot_verified=false`。不能据此宣称 Guest 启动、KVM 可用、Host 设备空闲或停止后 I/O 已释放。

库存不是原子快照，也不解析宿主机挂载、打开句柄或文件身份。`storage_observation_complete` 始终为 false。`resource_references_complete` 仅说明已支持的 XML 引用解析完整，不能直接转换为生产租约授权。默认测试域包含的旧格式磁盘或不支持元素可能产生 unknown，这是预期的保守结果。

控制器目前使用具体 `MockHypervisor`，没有可直接挂接的通用 Hypervisor trait。本 crate 暂不更改控制器状态机，亦不提供物理执行器；后续须先完成 Host 实态 reconciliation 和资源身份解析，再讨论控制器集成。

## 构建和验证

默认功能不链接 libvirt，离线测试可以运行。未编译 native 功能时，lab 明确返回 skipped，库存返回 `NativeUnavailable`。

```bash
cargo test -p elastic-virt-libvirt --locked --offline
```

显式 `native-libvirt` 功能使用 `virt=0.4.3`、`virt-sys=0.3.1`。原生依赖为 libvirt ≥ 6.0 的共享库与 pkg-config 元数据，不需要运行任何 libvirt daemon。Cargo.lock 固定 `uuid=1.11.0`，保持依赖与项目 Rust 1.85 基线兼容；更新锁文件时须复核 MSRV。

本轮使用已经验签、仅解包在 `/tmp` 的 libvirt 12.8.0，不安装系统包。以下目录是本机临时验收路径：

```bash
CARGO_HOME=/tmp/elasticpc-next-cargo-home \
PKG_CONFIG_PATH=/tmp/elasticpc-next-runtime/root/usr/lib/pkgconfig \
LD_LIBRARY_PATH=/tmp/elasticpc-next-runtime/root/usr/lib \
cargo test -p elastic-virt-libvirt --features native-libvirt --locked --offline
```

原生测试通过 14 项，其中包含真实 TEST driver 的只读库存和生命周期调用；默认功能包含 13 项测试。生命周期报告有 9 项检查。此处计数仅对应本 crate。

API 和行为依据：[virt 0.4.3 Connect](https://docs.rs/virt/0.4.3/virt/connect/struct.Connect.html)、[libvirt test driver](https://www.libvirt.org/drvtest.html)、[当前与非活跃 XML 的语义](https://libvirt.org/html/libvirt-libvirt-domain.html#virDomainGetXMLDesc)、[Domain XML 与 backingStore](https://www.libvirt.org/formatdomain.html)。构建同时核对了官方 crates.io 发布的 `virt`、`virt-sys` 源码及其 `build.rs`。
