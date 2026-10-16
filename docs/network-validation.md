# 网络实测与 Guest 接入策略

2026-10-14 只读采集与有界 Tailscale ping；没有修改路由、防火墙、代理、Tailscale 登录身份或生产服务。

## 已有证据

- 当前默认出口为 Wi-Fi `wlp4s0`，有线口 `enp2s0` 无载波。不能将普通以太网桥接方案直接套到当前无线接口。
- Host `tailscale netcheck`：UDP 可用，IPv4 可用，IPv6 未连通，MappingVariesByDestIP=false，发现 UPnP/NAT-PMP/PCP。发现协议支持不等于已创建且验证了映射。
- 自定义 DERP 探测延迟约 7.3 ms，作为当前回退路径保留。
- 两个在线对端的有界探测中，一个从 DERP 转为 UDP 直连，最后一次约 6 ms；另一个无响应，原因未确认。原始结果见 `reports/tailscale-host-direct-probes.json`。不能把 status 的 Relay 字段当作当前流量必然经过中继的证据。
- netcheck 的 HTTPS 请求使用本机 127.0.0.1:7890 代理；这不证明 Tailscale UDP 数据流也经过该代理，更不代表 Guest 自动继承代理或 Exit Node。

## 具体实施选择

1. Wi-Fi 模式优先采用独立 Guest 子网和内核 NAT，避免将高吞吐远程桌面长期放在额外的用户态 NAT 层后面。选择子网前检查所有路由表，避开局域网、Docker、K3s、Tailscale/已接受路由；当前已存在 192.168.31.0/24、172.17.0.0/16、10.42.0.0/24。
2. Host 与各 Guest 使用独立 Tailscale 身份。Trial 副本首次联网前处理从原系统复制的节点身份，不并行运行重复身份实例。
3. Guest 网络建立后实测 UDP 出站、DNS 和 direct/relay；先观察 NAT 行为，不能因存在 NAT 就盲目打开全部防火墙或暴露管理端口。
4. 若 Guest 直连受嵌套 NAT 限制，再比较 Host 子网路由方案。路由发布与 tailnet 审批单独实施，不自动将所有内网子网暴露给远端。
5. 有线联网时可另测桥接；不使用会切断当前 Wi-Fi 的桥接变更。Host 作为子网路由器时，再按网卡支持情况评估 UDP GRO 转发；它不是当前普通客户端的必需修改。
6. 吞吐验收需对端测试服务和明确目标，当前仅有连接路径及探测延迟证据，没有吞吐结论。

这些是基于当前 Host 的策略，Guest 尚未联网验收。当前无需为已能直连的 Host 强行修改配置。

依据：[libvirt 网络与无线桥接限制](https://wiki.libvirt.org/Networking.html)、[Tailscale 性能建议](https://tailscale.com/docs/reference/best-practices/performance)。
