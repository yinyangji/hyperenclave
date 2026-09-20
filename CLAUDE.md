# CLAUDE.md — Claude Code 项目指令

@AGENTS.md

## Claude Code 补充说明

- 遵循 `AGENTS.md` 的全部约束与规范；逐步操作流程见 `SKILL.md`。
- 本仓库是 `no_std` 裸机 hypervisor（RustMonitor），不是普通应用项目：没有文件系统、没有线程库、panic 不可恢复。生成代码前先确认目标模块是否运行在 monitor mode。
- 修改 `src/arch/x86_64/intel/` 后必须检查 `src/arch/x86_64/amd/` 是否需要同步修改，反之亦然。
- 运行构建/检查命令时注意 `VENDOR` 与 `SME` 参数的组合约束（`SME=on` 仅 `VENDOR=amd` 合法）。
