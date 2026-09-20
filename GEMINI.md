# GEMINI.md — Gemini CLI 项目指令

> **开始任何任务前，先完整阅读 `AGENTS.md`**（本仓库 AI 开发唯一真实源：项目约束、构建命令、代码规范、架构地图），逐步操作流程见 `SKILL.md`。以下为要点摘要：

## 要点摘要

- **项目**：HyperEnclave — Rust 编写的 `no_std` 裸机 hypervisor（TEE 安全监控器），运行于 VMX root / SVM host mode。
- **硬约束**：只用 `core`/`alloc`；panic 不可恢复，错误用 `hv_result_err!`；页表代码经形式化验证，改动需谨慎；Intel/AMD 两套实现需同步。
- **构建**：`make elf VENDOR=intel|amd [SME=on|off]`（Makefile 无 build 目标，构建目标是 elf）；`SME=on` 仅限 `VENDOR=amd`。
- **质量门禁**：`make format` → `make clippy` → `make test`，提交前必须全绿。
- **勿动**：`rust-toolchain`、`x86_64.json`、`linker.lds`、`lib/libtpm.a`、`VERSION`。
- **新 `.rs` 文件**必须带 Apache-2.0 license 头（照抄现有文件）。
