# HyperGPU 功能差距分析

> 本文对照 HyperGPU 设计目标（`docs/arch-images/io.png`、`isolate.png`、`hypergpu-design.png`、
> `hypergpu-trust.png`）与当前仓库实现，逐项梳理已落地内容、尚存差距及后续优先级。
>
> 分析日期：2026-10-12  
> 基准分支：master（commit e20740b 之后 + 本会话新增代码）

---

## 一、已实现（本会话落地）

### 1.1 设备识别层（`src/device/`）

| 模块 | 状态 | 说明 |
|------|------|------|
| `bdf.rs` | ✅ 完成 | BDF 16-bit 路由 ID；`ecam_offset()`；Display `bb:dd.f`；单测全绿 |
| `ident.rs` | ✅ 完成 | class 码 + vendor 白名单 + BAR ≥256MB 三重判定；Intel 0x8086 显式排除；编译期 `HE_PROTECT_BDF`/`HE_EXCLUDE_BDF` 覆盖 |
| `pci.rs` | ✅ 完成 | `ConfigAccess`/`ConfigWriteAccess` trait；`decode_bars_with_size`（write-all-ones sizing）；`probe_ident` |
| `domain.rs` | ✅ 完成 | `DeviceDomain`（per-BDF IOMMU 域）；空页表 = fail-closed；`map_range`/`clear`/`can_access` |
| `policy.rs` | ✅ 完成 | `DeviceAction::Protect/Passthrough`；`ProtectedDevice`；`bar_contains` |
| `mod.rs` | ✅ 完成 | `DeviceManager::enumerate/assign/release/program_iommu`；`DEVICE_MANAGER: LateInit`；`init()` |

### 1.2 控制面——页表层（GPM sinkhole）

| 组件 | 状态 | 说明 |
|------|------|------|
| `memory/mod.rs` | ✅ 完成 | `ff_page_paddr()`：运行时分配全 0xFF 帧（无 2MB 静态 `.data` 膨胀） |
| `cell.rs` | ✅ 完成 | `insert_gpm_region_with_device_holes()`：GPM 建立时绕开设备孔洞；`insert_device_sinkholes()`：将 config 页 + BAR 映射到 FF sinkhole |

### 1.3 控制面——PIO 层（0xCF8/0xCFC 拦截）

| 组件 | 状态 | 说明 |
|------|------|------|
| `arch/x86_64/pio.rs` | ✅ 完成 | `PioAction::ConfigSpace`；完整的 CONFIG_ADDRESS 锁存与 CONFIG_DATA 仿真；受保护设备读返回全 1/写丢弃；非受保护设备透传真实 PIO；`handle_pio` 带访问宽度参数 |
| `arch/x86_64/pci_cfg.rs` | ✅ 完成 | `LegacyPciConfig` 实现 `ConfigAccess + ConfigWriteAccess`；`decode_config_address` |

### 1.4 数据面——per-BDF IOMMU 域

| 组件 | 状态 | 说明 |
|------|------|------|
| `iommu/mod.rs` | ✅ 完成 | `GenericIommu::set_device_page_table` 加入 trait |
| `intel/vtd.rs` | ✅ 完成 | `set_device_context`：单条 context entry 精确覆写 + clflush；`set_device_page_table`：domain ≠ default 校验、context 缓存 + IOTLB 失效 |
| `amd/iommu.rs` | ✅ 完成 | 命令缓冲（ComLen=256）+ `INVALIDATE_DEVTAB_ENTRY`/`INVALIDATE_IOMMU_PAGES`/`COMPLETION_WAIT`（带超时，SME C-bit 一致）；DTE 写入独立 DomainID，避免 IOTLB 跨设备串用 |

### 1.5 基础设施

| 组件 | 状态 |
|------|------|
| `build.rs` HE_PROTECT/EXCLUDE_BDF | ✅ 完成 |
| `sync/late_init.rs` `try_get()` | ✅ 完成 |
| `hypercall/device.rs` 设备 hypercall（0x50–0x53） | ✅ 完成 |
| Intel/AMD `ecam_base()` | ✅ 完成 |
| `arch/x86_64/mod.rs` re-export | ✅ 完成 |

---

## 二、差距清单

下表按实现难度与功能阻断程度分级（🔴 阻断 / 🟠 重要 / 🟡 增强 / ⚪ 长期）。

### G1 — `PerCpu::pci_cfg_addr` 字段（✅ 已存在）

**现状**：`pio.rs::handle_config_pio` 读写 `cpu.pci_cfg_addr`，但 `src/percpu.rs` 的 `PerCpu` 结构体尚未添加此字段。  
**影响**：编译报错，控制面 PIO 路径无法工作。  
**修复**：在 `PerCpu` 添加 `pub pci_cfg_addr: u32`，初始化为 0。约 5 行。

---

### G2 — AMD IOMMU 命令缓冲与失效命令（✅ 已完成）

**现状**：`amd/iommu.rs` 的 `set_device_page_table` 只更新了 device table entry 的页表根指针，**没有提交任何失效命令**。IOMMU 硬件不会自动感知变更，旧 TLB 项仍有效，隔离不生效。

`IommuMmioRegion` 结构体包含 `cmd_buf_base`/`cmd_buf_head`/`cmd_buf_tail` 寄存器字段，但当前 `Iommu::new()` 从未分配命令缓冲区、从未配置 `CMDPTR_BASE`。

**需要补齐**：
1. `Iommu::new()` 中分配 4KB 命令缓冲帧，写入 `cmd_buf_base`（`SIZE=0`=4KB，`ptr=帧物理地址`）
2. 实现最小命令提交函数 `submit_cmd(inner, cmd: [u64; 2])`：写 tail，等待 head 追上（spin loop）
3. `set_device_page_table` 之后提交：  
   - `INVALIDATE_DEVTAB_ENTRY`（DeviceID = bdf.raw()）  
   - `INVALIDATE_IOMMU_PAGES`（domainId, S=1, PDE=1, GN=0）  
   - `COMPLETION_WAIT`（轮询 completion store / 自旋等待 status bit）

**参考**：AMD IOMMU spec §3.3.2，命令格式 §2.2.3。

---

### G3 — 设备分配 hypercall（✅ 已完成，ABI 见下方“落地 ABI”）

**现状**：`hypercall/mod.rs` 的 `HyperCallCode` 枚举中没有 `DeviceList`(0x50)、`DeviceAssign`(0x51)、`DeviceRelease`(0x52)。`DeviceManager::assign/release` 方法已经实现，但没有用户态入口。  
**影响**：宿主侧 VFIO 无法通过 hypercall 让 monitor 把设备绑定给某个 VM。

**需要补齐**：
1. `HyperCallCode` 添加三个枚举值（Supervisor 级 < 0x4000_0000）
2. `validate_state` 登记为 `CpuState::HvEnabled`
3. dispatch `match` 分支 → `hypercall/device.rs`
4. 实现三个 handler：
   - `DeviceList`：把 `DEVICE_MANAGER.devices()` 序列化写入 `GuestPtr` 缓冲（BDF + vendor/device + domain_id + 状态）
   - `DeviceAssign(bdf, handle, ranges_ptr, count)`：校验每段在 normal world → `manager.assign()`
   - `DeviceRelease(bdf)`：`manager.release()`

### 落地 ABI（G3 实现结果，与最初草案不同）

VM 内存在物理上几乎从不连续（例如 1GB 大页拼出的 100GB 虚机就是约 100 段），一次性传固定数组的
`DeviceAssign` 表达不了，因此改为与 VFIO 同构的“认领 + 分批映射”：

| code | 名称 | 参数 | 语义 |
|---|---|---|---|
| `0x50` | `DeviceList` | arg0 = `DeviceListBuf*` | 最多 32 条；`total` 给出真实数量 |
| `0x51` | `DeviceAssign` | arg0 = BDF，arg1 = owner handle | 认领设备，域仍为空（fail-closed） |
| `0x52` | `DeviceRelease` | arg0 = BDF | 清空映射、解除认领、刷新 IOTLB |
| `0x53` | `DeviceMapDma` | arg0 = `DeviceDmaMapReq*` | 每批 ≤64 条 `{iova, paddr, size}`；handle 必须与当前 owner 一致；整批原子生效 |

- 每段 `paddr` 必须完整落在单个 normal-world 区间内，且不触碰 CMR（EPC 现在或将来所在）——
  monitor、EPC、其他设备 MMIO 均无法被映射。跨两个相邻区间的范围会被拒绝，由调用方拆分。
- 预期内失败（设备不存在/已占用/范围非法）以负 errno 写回返回寄存器，不再走 `HvError` 路径
  （后者会向宿主内核注入 #GP）；坏指针仍按原有 `GuestPtr` 语义产生异常。
- 结构体布局由 `hypercall/device.rs` 的单测锁定（驱动侧需按同一布局实现）。
- 尚缺：`DeviceUnmapDma`（按段撤销，供 VM 内存热拔插）；当前只能整体 `DeviceRelease`。

---

### G4 — 嵌套虚拟化 / VMCS Shadowing 缺失（🔴 CVM 功能性缺失）

**现状**：`cpuid.rs` 主动清除宿主可见的 VMX/SVM 能力位（leaf 1 + leaf 0x8000_0001），`msr/policy.rs` 屏蔽整个 `IA32_VMX_*` MSR 区域。因此 `kvm_intel`/`kvm_amd` 无法加载，宿主**根本无法创建任何 VM**，"允许宿主创建 CVM 直通调用显卡"的根本前提不成立。

**设计**：HyperGPU 的 `isolate.png` 图示明确包含 eVMCS（VMX Enlightened VMCS）路径，使宿主 KVM 跑在 L1 guest mode 且能高效管理 L2 VM（CVM）。这是 L0 hypervisor 最复杂的部分。

**方案层次**：
- **最小可行**：恢复 CPUID VMX/SVM 位 + MSR，允许宿主加载 KVM；未实现 VMCS shadowing 时每次 L2 VM-exit 都会陷入 L0，性能极差但功能正确。风险：VMXON/SVMOFF 指令目前无条件 exit，需要在 vmexit 分发里正确处理（允许 L1 执行 VMXON、维护 nested vmx state machine）。
- **完整实现**：Intel eVMCS / VMCS Shadowing；AMD SVM nested（已有 `npt.rs`，AMD 嵌套 SVM 相对 Intel 简单）。估算：Intel VMCS shadowing 4000-8000 行，AMD 嵌套 SVM 2000-3000 行。

**本会话不触碰此项**（已在计划中标注为范围外，文档在此登记差距）。

---

### G5 — IOMMU DMA 故障日志未处理（🟠 可观测性缺失）

**现状**：受保护但未分配的设备发起 DMA 时，IOMMU 会生成故障事件写入故障日志（Intel：Fault Recording Register / Fault Event Interrupt；AMD：Event Log Buffer），但代码没有读取或打印这些记录。  
**影响**：真机上无法确认 DMA 隔离确实在发生，调试困难。  
**修复**：
- Intel `vtd.rs`：在 `init()` 末尾（或定期从 vmexit 触发）读取 `fault_status` 寄存器，如果 PPF/PFO 置位，遍历 Fault Recording Register 组打印 BDF + fault address + type，然后清 PPF。
- AMD `iommu.rs`：实现 Event Log 轮询，解码 IO_PAGE_FAULT 事件打印 DeviceID + address。

---

### G6 — `policy_stats.rs` 未扩展设备隔离计数器（🟠 可观测性缺失）

**现状**：`GetPolicyStats` hypercall 已实现，但 `PolicyStatsSnapshot` 中无设备相关计数器。  
**需要添加**：
- `config_space_deny_count: u64`（PIO 路径每次拦截 +1）
- `bar_deny_count: u64`（EPT/NPT violation 落在 sinkhole 帧 +1，可选）
- `dma_fault_count: u64`（从 IOMMU 故障日志读取并累计）

---

### G7 — Map/Release 后 IOTLB 失效（✅ 已完成：在域锁内同步 IOMMU）

**现状**：`program_iommu()` 在初始化时为每个受保护设备设置 per-BDF 页表并做了失效。但之后 `DeviceAssign` 调用 `domain.map_range()` 修改了 IOMMU 页表，并**没有再次失效 IOTLB**。  
**影响**：在 assign 前已发起 DMA 但被 fault 的设备（不太可能但理论上），或 Release 后重新 assign，旧 TLB 项可能仍有效。  
**修复**：在 `DeviceAssign`/`DeviceRelease` hypercall handler 中，`manager.assign()/release()` 成功后，对每个 IOMMU 调用一次局部失效（domain-specific IOTLB flush）。

---

### G8 — ARM / 鲲鹏 EL2 后端缺失（⚪ 平台扩展）

**现状**：整个 `src/arch/` 只有 `x86_64` 子树。用户首要目标是鲲鹏平台。  
**需要新增**：
- `src/arch/aarch64/` 树：EL2 入口（`entry.S` 的 `hvc`/`smc` 路径）、stage-2 页表（VTTBR_EL2 / VTCR_EL2）、SMMUv3（`set_device_page_table` 等价实现）、GICv3（VM 中断注入）、PSCI（CPU 热插拔 / reset）
- `x86_64.json` 对应 `aarch64.json` 自定义 target
- 设备枚举：ARM 平台无 0xCF8/0xCFC，枚举须走 ECAM（从 ACPI MCFG 或 DT 取基址）或 SMMU 的 StreamID 表

**规模估算**：最小 EL2 后端（bare bones CPU 隔离 + SMMUv3 passthrough 设置） ≈ 6000-10000 行。

---

### G9 — PCIe 链路加密（SM4-CTR / SM4-GCM）缺失（⚪ 机密计算增强）

**现状**：HyperGPU 设计图（`hypergpu-trust.png`）展示了 PCIe IDE（Integrity and Data Encryption，PCIe 5.0 规范 §6.33）保护主机 ↔ GPU 的物理链路，防止总线嗅探。  
**现状**：仓库无任何 PCIe IDE 配置代码。  
**实现路径**：需要 PCIe IDE extended capability 的枚举 + key programming，以及与 GPU 固件的 key 协商（SM2 ECDH 或 TLS 1.3 over DOE）。高度依赖卡型号固件支持，且当前仓库没有 MMIO 访问 PCIe extended capability 的基础设施。  
**短期建议**：本条作为长期演进项，先在设计文档中登记，不阻断当前隔离功能。

---

### G10 — GPU 度量与远程证明（⚪ 可信根扩展）

**现状**：HyperGPU 信任模型（`hypergpu-trust.png`）包含 GPU attestation（NVIDIA RIM / Confidential Computing attestation）将 GPU 纳入 TEE 的可信计算基。  
**现状**：仓库的 TPM 路径（`src/hypercall/tc.rs`）用于 CPU 侧度量，GPU 没有对应的度量钩子。  
**实现路径**：
- NVIDIA H100/H200：通过 NV-HOPPER confidential computing API（`nvmlDeviceGetConfComputeGpuCertificate`）获取 GPU attestation report，经 TPM quote 链接进整体 trust chain。
- 昇腾 / DCU：厂商各有不同 attestation 接口，需逐一适配。  
**短期建议**：与 G9 一起列入长期计划。

---

## 三、差距优先级汇总

| 编号 | 标题 | 严重程度 | 估算工作量 | 先决条件 |
|------|------|----------|----------|----------|
| G1 | `PerCpu::pci_cfg_addr` 字段 | ✅ 已存在 | — | — |
| G2 | AMD IOMMU 命令缓冲 + 失效 | ✅ 完成 | — | — |
| G3 | 设备分配 hypercall | ✅ 完成 | — | — |
| G7 | Map/Release 后 IOTLB 失效 | ✅ 完成 | — | — |
| G5 | IOMMU 故障日志读取 | 🟠 可观测 | ~80 行 | 无 |
| G6 | PolicyStats 设备计数器 | 🟠 可观测 | ~40 行 | G3 |
| G4 | 嵌套虚拟化 / VMCS Shadowing | 🔴 CVM 前提 | 4000–10000 行 | 无（独立大模块） |
| G8 | ARM/鲲鹏 EL2 后端 | ⚪ 平台扩展 | 6000–10000 行 | G4 设计模式参考 |
| G9 | PCIe IDE 链路加密 | ⚪ 机密增强 | 未知（依赖固件） | G3 |
| G10 | GPU 度量与远程证明 | ⚪ 可信根 | 未知（依赖厂商 API） | G9 |

---

## 四、近期可验证的交付物

以下工作在本机（无真机）即可完成并通过质量门禁：

```bash
# 目标：G1 + G2 + G3 实现后，以下应全部通过
make format-check
make clippy VENDOR=intel SME=off
make clippy VENDOR=amd  SME=on
make elf VENDOR=intel SME=off LOG=info
make elf VENDOR=amd  SME=on
make test   # 包含 src/device/ 全部单测 + pio 单测 + AMD IOMMU 命令缓冲单测
```

真机验证（需 hyperenclave-driver + Kunpeng/x86 + GRUB 预留内存）：
- `dmesg | grep HyperEnclave`：确认枚举到正确数量的加速卡
- `lspci` on host：看不到受保护卡（vendor ID 0xFFFF 或不出现）
- 在 VM 内通过 VFIO passthrough 运行 `nvidia-smi`：正常显示 H200

---

## 五、与 HyperGPU 论文/分享对照

基于 `docs/arch-images/` 中的架构图做逐项对照：

| HyperGPU 能力 | 对应本仓库实现状态 |
|---------------|-------------------|
| L0 控制面——BAR sinkhole（EPT/NPT） | ✅ `cell.rs` GPM hole + FF sinkhole |
| L0 控制面——PIO 0xCF8/CFC 拦截 | ✅ `pio.rs` ConfigSpace action |
| L0 数据面——per-device IOMMU 域（Intel VT-d） | ✅ `vtd.rs` set_device_page_table |
| L0 数据面——per-device IOMMU 域（AMD-Vi） | ✅ `amd/iommu.rs` 命令缓冲 + 独立 DomainID |
| L0 设备枚举与分类 | ✅ `device/ident.rs` + `device/mod.rs` |
| L0↔L1 分配 ABI（hypercall） | ✅ `hypercall/device.rs`（0x50–0x53） |
| L1 嵌套虚拟化（eVMCS / KVM 可运行） | ❌ CPUID/MSR 隐藏尚未恢复（G4） |
| L1 VFIO 直通给 L2 VM | ❌ 依赖 G4 |
| PCIe IDE 链路加密 | ❌ 未实现（G9） |
| GPU attestation 纳入 trust chain | ❌ 未实现（G10） |
| ARM/鲲鹏 SMMUv3 后端 | ❌ 未实现（G8） |
