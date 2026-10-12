# SKILL.md — HyperEnclave 开发技能索引

> 前置阅读：`AGENTS.md`（项目约束、构建命令、代码规范，唯一真实源）。
>
> 各技能的完整步骤以 Claude Code 标准格式存放在 `.claude/skills/<name>/SKILL.md`
> （YAML frontmatter：`name` + `description`）。Claude Code 会按任务自动加载；
> 其他 AI 工具（Gemini、Cursor、Copilot 等）请按下表直接阅读对应文件。

| 技能 | 文件 | 何时使用 |
|---|---|---|
| 构建与验证 | `.claude/skills/build-verify/SKILL.md` | 任何代码改动后、提交前 |
| 新增 hypercall | `.claude/skills/add-hypercall/SKILL.md` | 新增或修改 `src/hypercall/` 接口 |
| 修改 TCB 内存代码 | `.claude/skills/tcb-memory-change/SKILL.md` | 页表、GPM/EPT/NPT、`cell.rs`、`src/memory/` |
| IOMMU 与设备隔离 | `.claude/skills/iommu-device-isolation/SKILL.md` | 算力卡隔离、IOMMU、`src/device/`、0xCF8/0xCFC |
| 真机调试 | `.claude/skills/debug-on-hardware/SKILL.md` | 真机启动失败、日志、真机验证 |

审查角色（Claude Code subagent，其他工具可作为审查清单使用）：

| 角色 | 文件 |
|---|---|
| Intel / AMD 成对检查 | `.claude/agents/vendor-parity-reviewer.md` |
| TCB 安全审查 | `.claude/agents/tcb-security-reviewer.md` |
| 双厂商门禁执行 | `.claude/agents/gate-runner.md` |

## 提交前检查清单

- [ ] `make format` 已执行，`make format-check` 无输出
- [ ] `make clippy`（intel 与 amd 两个配置）无新增 warning
- [ ] `make test` 通过；`make elf` 两个厂商均构建成功
- [ ] 新文件带 Apache-2.0 license 头
- [ ] 新增 `unsafe` 有 `// SAFETY:` 注释
- [ ] 未引入 `std` 依赖；未改动 `rust-toolchain`/`x86_64.json`/`linker.lds`
- [ ] Intel 与 AMD 两侧实现同步
- [ ] 未提交 `target/` 产物
