# 发现器 fixture 验收

输入位于 `fixtures/`，样例来源说明见 `docs/hardware-fixture-notes.md`。执行 `cargo test -p elastic-probe --locked --offline`。

断言包括：GPU/HDA 完整组、flr/bus、Manjaro root 已挂载、T7 ESP、不在 BootOrder 的 Boot0002；真实未知 DRM 归属保留未知，单独的合成拓扑验证 card 到 PCI 的解析；Windows 在第四分区的反例防止按 NTFS 顺序猜测；Intel 与无显示器以及缺工具环境仍返回结构化结果。
