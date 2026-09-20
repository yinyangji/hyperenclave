# AGENTS.md — HyperEnclave AI 开发指南

> 本文件是本仓库 AI 辅助开发的**唯一真实源**（Single Source of Truth）。
> `CLAUDE.md`、`GEMINI.md`、`.cursor/rules/`、`.github/copilot-instructions.md`、`SKILL.md` 均指向本文件。修改项目约定时只改这里。

## 1. 项目是什么

HyperEnclave 是一个开源、跨平台的可信执行环境（TEE），核心是一个用 Rust 编写的轻量级 hypervisor（学术论文中称 **RustMonitor**，Cargo 包名 `rust-hypervisor`）。它运行于 x86 虚拟化扩展的最高特权级（Intel VMX root mode / AMD SVM host mode），通过软件方式提供 SGX 兼容的 Enclave 抽象，信任根（RoT）构建在 TPM 之上而非 CPU 厂商。

架构文档见 `docs/rustmonitor-architecture.md`。

## 2. 关键约束（改代码前必读）

1. **`no_std` 环境**：这是裸机 hypervisor，只能使用 `core` 和 `alloc`，**禁止**引入任何 `std` 依赖。`#![cfg_attr(not(test), no_std)]`（见 `src/main.rs`）——仅在 `cargo test` 宿主机测试时可隐式使用 std。
2. **安全关键代码（TCB）**：本 hypervisor 是整个 TEE 的可信计算基，页表模块已经过形式化验证（ASPLOS'24，CertiK/Coq）。对 `src/memory/`、页表相关代码的任何改动都可能破坏已证明的安全不变量，须格外谨慎。
3. **panic 不可恢复**：代码运行在最高特权级，panic 会导致整机崩溃。避免新增 `unwrap()`/`expect()`/`panic!()`；错误用 `hv_result_err!` 宏返回 `HvResult`。
4. **自定义编译目标**：使用 `x86_64.json` 自定义 target 和 `linker.lds` 链接脚本，nightly 工具链 `nightly-2025-11-05`。**不要**修改 `rust-toolchain`、`x86_64.json`、`linker.lds`，除非任务明确要求。
5. **厂商差异**：Intel（VMX/EPT/VT-d）与 AMD/Hygon（SVM/NPT/AMD-Vi）是两套并行实现，分别位于 `src/arch/x86_64/intel/` 与 `src/arch/x86_64/amd/`。架构无关的逻辑必须放公共层（`src/` 顶层或 `src/arch/x86_64/vmm.rs`），通过 feature 门控 `#[cfg(feature = "intel")]` / `#[cfg(feature = "amd")]` 选择。
6. **超时/阻塞**：monitor mode 下没有调度器，`spin_loop()` 忙等是唯一的同步等待方式（见 `src/main.rs` 的 `wait_for_other_completed`）。

## 3. 构建与验证命令

```bash
# 构建（默认 VENDOR=amd SME=on INTR=on；SME=on 仅在 VENDOR=amd 时合法）
# 注意：Makefile 没有 build 目标，构建目标是 elf（或直接 make，其包含 githooks+elf）
make elf VENDOR=intel SME=off LOG=warn
make elf VENDOR=amd  SME=on

# 一键构建并安装到 /lib/firmware（需 sudo，自动探测 SME）
bash -x scripts/build_and_install_hyperenclave.sh [Intel|AMD|Hygon]

# 质量门禁（提交前必须全绿）
make format-check   # rustfmt 检查（pre-commit hook 会强制执行）
make clippy         # clippy 检查
make test           # 宿主机单元测试（cargo test）

make format         # 自动格式化
make disasm         # 查看上次构建的反汇编
make install        # sudo cp ELF 到 /lib/firmware/rust-hypervisor-<VENDOR>
```

> 环境准备：需要 `rustup component add rust-src --toolchain nightly-2025-11-05`（构建用 `-Z build-std`），首次构建 rustup 会自动下载工具链。

Makefile 参数：

| 参数 | 取值 | 说明 |
|------|------|------|
| `VENDOR` | `intel` \| `amd` | CPU 厂商；Hygon/Zhaoxin 走 `amd` |
| `SME` | `on` \| `off` | AMD 内存加密；`on` 时强制 `VENDOR=amd` |
| `STATS` | `on` \| `off` | Enclave 性能统计 |
| `INTR` | `on` \| `off` | Enclave 运行期间响应中断（`enclave_interrupt` feature） |
| `LOG` | `off\|error\|warn\|info\|debug\|trace` | 编译期日志级别 |

注意：`MODE` 固定为 release，不支持 debug 构建。

Cargo features（`Cargo.toml`）：`intel`、`amd`、`sme`（隐含 amd）、`stats`、`enclave_interrupt`、`epc48`~`epc384`（EPC 大小档位）。

## 4. 代码规范

1. **License 头**：每个 `.rs` 源文件开头必须带 Apache-2.0 版权头，格式照抄任意现有文件（`Copyright (C) 2023 Ant Group CO., Ltd. All rights reserved.` + 许可证文本）。
2. **格式**：`rustfmt` 强制。pre-commit hook（`.githooks/pre-commit`，`make githooks` 安装）会拒绝未格式化代码；提交前跑 `make format`。
3. **注释语言**：代码注释与 doc comment 用**英文**，跟随现有代码风格；注释密度与周边代码保持一致。
4. **unsafe**：新增 `unsafe` 块须带 `// SAFETY:` 注释说明为何成立（新代码标准；现有部分代码未补齐，不要模仿旧欠账）。
5. **错误处理**：统一用 `hv_result_err!(errno)` / `hv_result_err!(errno, msg)` 返回 `HvResult`；hypercall 层用 `hypercall_hv_err_result!`。禁止在可恢复路径上 `unwrap()`。
6. **日志**：用 `info!`/`warn!`/`error!`/`debug!`/`trace!` 宏（`log` crate）；注意日志是编译期裁剪的，昂贵格式化用对应级别宏包裹。
7. **依赖**：新增依赖优先 `default-features = false`（no_std 兼容）；本地 crate 在 `crates/`（`libvmm`、`uart_16550`、`yogcrypt`）。

## 5. 架构地图

```
src/
├── main.rs              # 入口：多核初始化（primary_init_early → PerCpu::init
│                        #   → primary_init_late → activate_vmm → vmlaunch/vmrun）
├── cell.rs              # Root Cell：三套页表 GPM(EPT/NPT)/HVM(自身)/DMA(IOMMU)
├── percpu.rs            # PerCpu：CPU 状态机 HvDisabled/HvEnabled/EnclaveRunning
├── config.rs / header.rs# 驱动传入的 HvSystemConfig / HvHeader（内存布局、EPC 区间）
├── hypercall/           # Hypercall 分发：HyperCallCode 枚举 + 特权级校验
│   ├── enclave.rs       # Enclave 生命周期 hypercall
│   └── tc.rs            # TPM/密码模块接口（链接 lib/libtpm.a）
├── enclave/             # Enclave 核心逻辑
│   ├── manager.rs       # Enclave 管理器
│   ├── epcm.rs          # EPC 页元数据
│   ├── edmm.rs          # 动态内存管理
│   ├── shared_mem.rs    # Marshalling Buffer / 共享内存
│   ├── reclaim.rs       # EPC 页回收（换出/换入）
│   └── report.rs / measure.rs  # 度量与远程证明
├── iommu/mod.rs         # IOMMU 抽象：init 时配置 DMA 页表并启用
├── memory/              # 通用内存框架：paging.rs(页表抽象)/frame.rs/heap.rs/mmio.rs
├── intervaltree.rs      # 区间树（共享内存/normal world 校验）
└── arch/x86_64/
    ├── vmm.rs           # VmExit 公共处理（CPUID 伪装/hypercall 分发/MSR）
    ├── entry.rs         # arch_entry 汇编入口（栈切换）
    ├── intel/           # VMX 实现：vcpu.rs(VMCS)/vmexit.rs/ept.rs/vtd.rs
    └── amd/             # SVM 实现：vcpu.rs(VMCB)/vmexit.rs/npt.rs/iommu.rs

crates/
├── libvmm/              # VMX/SVM 硬件结构与指令封装（独立 no_std crate）
├── yogcrypt/            # 国密 SM2/SM3/SM4（证书签名用）
└── uart_16550/          # 串口驱动（日志输出）

lib/libtpm.a             # 预编译 TPM 库（build.rs 链接，版本须与 VERSION 匹配）
x86_64.json / linker.lds # 自定义编译目标与链接脚本（勿动）
```

关键数据流（快速定位）：

- **启动**：内核模块加载 → `arch_entry`（`src/arch/x86_64/entry.rs`）→ `main()`（`src/main.rs`）→ `activate_vmm` → Linux 被降级为 guest。
- **VM Exit**：硬件陷入 → `vmexit_handler`（`src/arch/x86_64/vmm.rs`）→ 按退出原因分发到 intel/amd 的 `vmexit.rs` 或公共 handler。
- **Hypercall**：`VMCALL`/`VMMCALL` → `VmExit::handle_hypercall` → `HyperCall::hypercall`（`src/hypercall/mod.rs`）→ 具体 handler。
- **Enclave 缺页**：EPT/NPT violation → `Enclave::handle_npt_violation`。

## 6. 常见任务

逐步操作指南见 `SKILL.md`。速查：

- **新增 hypercall**：`HyperCallCode` 加枚举值（编码规则：Supervisor 级 < `0x4000_0000`，User 级 ≥ `0x8000_0000`）→ `validate_state()` 登记 → dispatch match 加分支 → 实现 handler。
- **新增内存区域类型**：`src/memory/mod.rs` 的 `MemorySet`/`MemoryRegion`，注意 GPM/HVM/DMA 三套页表的一致性。
- **调试**：串口/`dmesg` 看日志；`make disasm` 看反汇编；真机需 hyperenclave-driver + Linux 5.10/5.4 内核。
- **真机启动**：GRUB 需 `memmap=<N>G\$0x100000000 iommu=off intremap=off no5lvl` 预留安全内存，详见 `README.md`。

## 7. 禁止事项

- 不要提交 `target/` 构建产物。
- 不要改动 `rust-toolchain`、`x86_64.json`、`linker.lds`、`lib/libtpm.a`、`VERSION`（除非任务明确要求）。
- 不要为 no_std 代码引入 `std` 或依赖 std 的 crate。
- 不要删除或绕过 license 头与 pre-commit hook。
- 不要在架构无关代码里直接引用 `intel::` / `amd::` 内部模块，一律走 `src/arch/x86_64/vmm.rs` 的 `vendor` re-export。
- 不要自行 `git push` / 提交，除非用户明确要求。

## 8. 参考资料

- `README.md` — 构建、部署、运行 TEE 应用的完整流程
- `docs/rustmonitor-architecture.md` — RustMonitor 架构设计文档
- `crates/libvmm/README.md` — libvmm 库说明
- 论文：HyperEnclave（USENIX ATC'22）、页表形式化验证（ASPLOS'24）
