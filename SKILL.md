---
name: hyperenclave-dev
description: HyperEnclave hypervisor 开发技能：构建验证、新增 hypercall、修改页表/IOMMU 代码的规范流程与安全检查清单。当任务涉及本仓库（rust-hypervisor）的代码修改、构建、调试时使用。
---

# SKILL.md — HyperEnclave 开发技能

> 前置阅读：`AGENTS.md`（项目约束、构建命令、代码规范）。本文件提供常见任务的逐步操作流程。

## 技能 1：构建与验证（任何代码修改后执行）

```bash
# 1. 格式化（必须先于提交）
make format

# 2. 静态检查（按目标厂商跑；两个厂商都改了就都跑）
make clippy VENDOR=intel SME=off
make clippy VENDOR=amd  SME=on

# 3. 单元测试
make test

# 4. 构建产物验证
make build VENDOR=intel SME=off LOG=warn
make build VENDOR=amd  SME=on  LOG=warn
```

**验收标准**：四步全部零 error。clippy 新增 warning 视为不通过。

## 技能 2：新增一个 Hypercall

以新增 `FooBar` 为例：

1. **定义调用码**（`src/hypercall/mod.rs`）：
   - Supervisor 级（仅内核模块可调）：编码 < `0x4000_0000`，紧邻现有同类码分配。
   - User 级（Enclave/用户态可调）：编码 ≥ `0x8000_0000`。
   - 加入 `HyperCallCode` 枚举（`numeric_enum!` 宏内）。
2. **登记合法 CPU 状态**：`HyperCallCode::validate_state()` 的 match 中加入 `FooBar`，参考同类 hypercall 允许的状态（如仅 `HvEnabled`、或含 `EnclaveRunning`）。
3. **路由**：`HyperCall::hypercall()` 的 dispatch match 中加 `HyperCallCode::FooBar => self.foo_bar(arg0, arg1)`。
4. **实现 handler**：
   - 参数来自 guest 寄存器（`self.cpu_data.vcpu.regs()`，约定 `rdi`=arg0、`rsi`=arg1）。
   - 返回 `HyperCallResult<...>`；失败用 `hypercall_hv_err_result!(EINVAL, "...")`。
   - 涉及 guest 内存的参数必须先校验：用 `vcpu.guest_page_table().query()` 查 GPA，再用 `cell::ROOT_CELL.is_valid_normal_world_gpaddr()` 确认落在 normal world 区间，绝不直接解引用用户提供的指针。
5. **测试**：跑技能 1 的完整流程；若 hyperenclave-driver 侧需要对应 ioctl，提醒用户驱动仓库需同步修改。

## 技能 3：修改页表相关代码（高危）

`src/memory/`、`src/arch/x86_64/*/ept.rs|npt.rs`、`src/cell.rs` 属于形式化验证覆盖或安全不变量区域：

1. 改动前先列出影响的不变量：GPA→HPA 映射唯一性、EPC 页仅映射进 Enclave GPT、GPM 中安全内存映射为 empty page 等。
2. Intel（EPT）与 AMD（NPT）必须同步修改，二者通过 `GenericPTE` trait 对齐（`src/memory/paging.rs`）。
3. 修改映射后必须考虑 TLB 一致性：调用对应页表的 `flush()`；SVM 侧还有 VMCB clean bits 需清理。
4. 新增映射路径必须验证目标物理地址不与 hypervisor/EPC 区域重叠（参考 `Cell::new_root` 中的 empty mapper 模式）。
5. 自查：能否证明"不可信方（Normal VM/Enclave/设备 DMA）无法通过新代码访问安全内存"？不能证明就不要提交。

## 技能 4：修改 IOMMU / 设备直通相关

`src/iommu/mod.rs`、`src/arch/x86_64/intel/vtd.rs`、`src/arch/x86_64/amd/iommu.rs`：

1. DMA 页表只允许映射 `dma_regions`（`Cell` 中标记 `MemFlags::DMA` 或 RMRR 覆盖的区域）。
2. 新增设备直通能力时，先确认该设备 DMA 无法触达 EPC/hypervisor 物理区间——这是 R-3 安全要求。
3. VT-d 的 root/context 表改动需 cache line flush（参考 `vtd.rs` 的 `flush_cpu_cache`）；AMD IOMMU 用命令缓冲区同步。

## 技能 5：调试运行问题

- **看日志**：真机上 `dmesg`（驱动把 hypervisor 日志转内核 ring buffer）；日志级别由构建参数 `LOG=` 决定，默认未开日志时可加 `LOG=debug` 重编。
- **看反汇编**：`make disasm`（objdump，Intel 语法）。
- **启动失败**：检查 GRUB 是否有 `memmap=...G$0x100000000 iommu=off intremap=off no5lvl`；`dmesg` 里逐 CPU 看 `Activating hypervisor on CPU N...` 是否全部出现。
- **版本不匹配报错**：`lib/libtpm.a` 的版本必须与 `VERSION` 文件一致（`main.rs` 的 `primary_init_late` 会校验），不一致时不要手改 VERSION 去"适配"，应提醒用户获取匹配的 libtpm。

## 提交前检查清单

- [ ] `make format` 已执行，`make format-check` 无输出
- [ ] `make clippy`（intel 与 amd 两个配置）无新增 warning
- [ ] `make test` 通过
- [ ] 新文件带 Apache-2.0 license 头
- [ ] 新增 `unsafe` 有 `// SAFETY:` 注释
- [ ] 未引入 `std` 依赖；未改动 `rust-toolchain`/`x86_64.json`/`linker.lds`
- [ ] Intel 与 AMD 两侧实现同步
- [ ] 未提交 `target/` 产物
