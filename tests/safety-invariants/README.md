# 存储与设备安全验收

运行：`cargo test -p elastic-planner --test mock_safety --locked --offline` 及 `cargo test -p elastic-planner --locked --offline`。

要求：整盘/子分区双向冲突；同 PARTUUID 双写冲突；IOMMU group 独占；Host 根/活动 swap 不可分配；观察不完整时拒绝；原子获取；start/stop 幂等；部分执行后的崩溃恢复；后端 I/O 仍在时不释放；基底指纹变化拒绝 overlay 恢复。

真实系统上无法由本模块阻止任意其他进程打开设备。生产 helper 与实际内核/进程/libvirt 对账属于后续门槛。
