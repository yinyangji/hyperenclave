# HyperEnclave Copilot 指令

本仓库是 HyperEnclave 的 Rust hypervisor（RustMonitor）——一个 `no_std` 裸机安全监控器，运行于 VMX root / SVM host mode。生成代码时遵循：

## 核心约束

- **no_std**：只用 `core` 和 `alloc`，禁止 `std`；新依赖需 `default-features = false`。
- **panic 不可恢复**：避免 `unwrap()`/`expect()`/`panic!()`；错误用 `hv_result_err!(errno)` 返回 `HvResult`。
- **安全关键（TCB）**：页表代码（`src/memory/`、`src/arch/x86_64/*/ept.rs|npt.rs`）经形式化验证，改动须保持安全不变量。
- **双厂商同步**：Intel（`src/arch/x86_64/intel/`）与 AMD（`src/arch/x86_64/amd/`）是平行实现，修改一侧需检查另一侧；公共逻辑放 `src/` 顶层。
- **guest 指针不可信**：处理 guest 传入地址必须先经 guest 页表查询（`vcpu.guest_page_table().query()`）和 `ROOT_CELL.is_valid_normal_world_gpaddr()` 校验。

## 代码规范

- 每个 `.rs` 文件带 Apache-2.0 license 头（照抄现有文件）。
- rustfmt 强制（`make format`）；注释用英文。
- 新增 `unsafe` 带 `// SAFETY:` 注释。
- 错误处理宏：`hv_result_err!` / `hypercall_hv_err_result!`。

## 常用命令

```bash
make build VENDOR=intel SME=off    # Intel 构建
make build VENDOR=amd  SME=on      # AMD 构建（SME=on 仅 VENDOR=amd 合法）
make format / make format-check    # 格式化 / 检查
make clippy                        # 静态检查
make test                          # 单元测试
```

勿改：`rust-toolchain`、`x86_64.json`、`linker.lds`、`lib/libtpm.a`、`VERSION`。

更多细节见 `AGENTS.md`（唯一真实源）与 `SKILL.md`（常见任务操作流程）。
