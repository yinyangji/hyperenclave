# CLAUDE.md — Claude Code 项目指令

@AGENTS.md

## Claude Code 补充说明

- 遵循 `AGENTS.md` 的全部约束与规范（唯一真实源）；本文件只放 Claude Code 专属内容。
- 本仓库是 `no_std` 裸机 hypervisor（RustMonitor），不是普通应用项目：没有文件系统、没有线程库、panic 不可恢复。生成代码前先确认目标模块是否运行在 monitor mode。
- 修改 `src/arch/x86_64/intel/` 后必须检查 `src/arch/x86_64/amd/` 是否需要同步修改，反之亦然。
- 运行构建/检查命令时注意 `VENDOR` 与 `SME` 参数的组合约束（`SME=on` 仅 `VENDOR=amd` 合法）。

## Skills（`.claude/skills/`，按任务自动加载）

| Skill | 何时使用 |
|---|---|
| `build-verify` | 任何代码改动后、提交前；跑双厂商质量门禁 |
| `add-hypercall` | 新增或修改 hypercall（`src/hypercall/`） |
| `tcb-memory-change` | 改页表 / GPM / EPT / NPT / `cell.rs` / `src/memory/` |
| `iommu-device-isolation` | 改算力卡隔离、IOMMU、`src/device/`、0xCF8/0xCFC 拦截 |
| `debug-on-hardware` | 真机启动失败、日志排查、真机验证隔离效果 |

## Subagents（`.claude/agents/`）

| Agent | 用途 |
|---|---|
| `vendor-parity-reviewer` | 只读：检查 Intel / AMD 两侧改动是否成对 |
| `tcb-security-reviewer` | 只读：TCB 安全审查（内存隔离、DMA、guest 输入、panic 路径） |
| `gate-runner` | 跑完整双厂商门禁，只回传简短结果 |

改动 `src/arch/x86_64/` 后主动使用 `vendor-parity-reviewer`；改动 TCB 代码后提交前使用 `tcb-security-reviewer`。

## 权限（`.claude/settings.json`）

门禁命令（`make format|clippy|test|elf`）免确认；编辑 `rust-toolchain`、`x86_64.json`、`linker.lds`、`VERSION`、`lib/` 以及 `git push`、`make install` 需用户确认；禁止 `git commit --no-verify`。
