# 嵌套虚拟化（Nested SVM，AMD）实现设计

> **文档定位**：本文是 AMD 侧嵌套虚拟化的实现级设计，与 Intel 侧 [nested-vmx-design.md](nested-vmx-design.md) 对称，共同关闭 G15（宿主正常管理自己创建的 VM）。目标形态为 HyperGPU 的 L0/L1/L2 三层：L0 = 本 monitor（SVM host mode），L1 = 降级后的 Host Linux（跑 KVM-amd），L2 = L1 创建的 VM（即 CVM）。
>
> **范围**：仅 AMD/Hygon SVM。Intel 侧见对应文档。两侧共用 `nested` cargo feature 与 Makefile `NESTED=on`，**默认关闭**；关闭时行为与现状逐位一致（SVM 在 CPUID 中隐藏、`EFER.SVME` 对 guest 写拒绝、VMRUN 等 #UD）。
>
> **为什么先做 AMD**：SVM 嵌套比 VMX 简单——VMCB 是普通内存结构（非 VMREAD/VMWRITE 访问），guest 的 VMCB 可直接按字段读写，无需影子 VMCS 的字段编码表；嵌套页表合成（nNPT）与 Intel 影子 EPT 同构，可共用框架。

---

## 0. 默认关闭 = 现状

`nested=off` 时与当前逐位一致（回归底线）：

- CPUID leaf 0x8000_0001 ECX 的 SVM 位（bit 2）继续清除（[cpuid.rs:349-350](../src/arch/x86_64/cpuid.rs#L349)）；
- MSR `EFER`（0xC000_0080）的 SVME 位对 guest 写入继续被过滤（[amd/vcpu.rs](../src/arch/x86_64/amd/vcpu.rs) `wrmsr_virt` 的 `EFER_WRITABLE` 不含 SVME）；
- MSR `VM_CR`（0xC001_0114）继续走 Emulate 读 0；
- guest 执行 VMRUN/VMLOAD/VMSAVE/STGI/CLGI：`EFER.SVME=0` → #UD，与裸机"无 SVM"一致。

`nested=on` 时打开 §3–§7 的机制。两维门控：编译期 `#[cfg(feature = "nested")]`（不编译即零成本）+ 运行期 per-vCPU `NestedSvmState`（未置 SVME 前一切如旧）。

## 1. 威胁模型与安全不变量

L1（降级 Linux 及其 KVM）**完全不可信**。嵌套只是给它多一层可管理的执行域，不改变信任边界。

| # | 不变量 | 机制 |
|---|---|---|
| I1 | 受保护加速卡对 L1 与 L2 同样不可见。设备隔离挖在 L0 的 GPM(NPT) 里；嵌套页表合成终结于 L0 GPM 查询，受保护 MMIO 解析到 all-ones sinkhole | C5 |
| I2 | 真实 IOMMU(AMD-Vi) 表只有 L0 编程；不向 L1 暴露虚拟 IOMMU → **L2 设备直通由 L0 的 device hypercall 代理**，不走 L1 的 VFIO（见 [hypergpu-gap-analysis.md](hypergpu-gap-analysis.md) CVM+GPU 章） | C1 |
| I3 | L0 的强制拦截不可被 L1 撤销：VMMCALL hypercall 通路、MSRPM/IOPM 策略位（含 0xCF8/0xCFC、PM1a、APMC）。VMCB02 的拦截位 = L0 位 **OR** L1 位，L1 只能增不能减 | C4 |
| I4 | L2 内存访问必过合成 nNPT（L1 nNPT ∘ L0 GPM），L1 无法借 L2 触达安全内存/EPC | C5 |
| I5 | L0 对 L1 既有约束保留：S3 拒绝、SMM 锁定、LA57 fail-fast、CR0/CR4 保留位校验 | 既有机制 |

不防御：侧信道、DoS、硬件攻击。L1 的 KVM 崩溃最多毁掉 L2。

## 2. 总体结构：VMCB01 / VMCB12 / VMCB02

```
          ┌───────────────────────── L0 (monitor, SVM host)
          │  VMCB01：跑 L1 的真实 VMCB（现状）
          │  VMCB02：跑 L2 的真实 VMCB（每 CPU 一个，L0 合并生成）
          │  nNPT  ：L1 nNPT ∘ L0 GPM 的合成表（每 L1-nCR3 一棵）
          └──────▲──────────────▲──────────┘
       #VMEXIT   │              │  VMRUN（L0 执行）
   ┌─────────────┴──┐   ┌───────┴──────────┐
   │ L1 (Linux+KVM) │   │ L2 (KVM 的 guest)│
   │ VMCB12：KVM 为 │   │ 看到的硬件 =     │
   │ L2 准备的 VMCB │   │ VMCB02 合成出的世界│
   └────────────────┘   └──────────────────┘
```

- **VMCB01**：L0 跑 L1 用的真实 VMCB（现有代码里的那个）；
- **VMCB12**：L1(KVM) 为 L2 准备的 VMCB。它是 L1 地址空间里一页普通内存，由 L0 按 SVM 语义直接读写（SVM 的优势：无需字段编码表，VMCB 布局固定）；
- **VMCB02**：L0 为跑 L2 准备的真实 VMCB，每 CPU 一份，由 C4 合并规则从 VMCB12 + L0 强制位生成。

与 Intel 的关键差异：SVM 没有 VMREAD/VMWRITE，`VMRUN` 单条指令携带 VMCB 物理地址。因此 L0 只需拦截 `VMRUN`，在其中读 L1 给的 VMCB12（guest 物理地址，经 L0 GPM 翻译到 HPA 后直接映射访问），合并出 VMCB02，执行真实 VMRUN，退出后把 L2 状态写回 VMCB12 并反射给 L1。

## 3. C1：能力暴露

### 3.1 CPUID

- **leaf 0x8000_0001 ECX bit 2 (SVM)**：`nested=on` 保留真实值；`off` 维持清除。这是 `apply_masks` 中唯一随 feature 翻转的 AMD 位。
- **leaf 0x8000_000A（SVM Revision and Feature Identification）**：现状未特殊处理（透传真实值）。`nested=on` 时需掩码：
  - EDX 特性位保留 **NP（nested paging，bit 0）、NRIPS（bit 3）**；
  - **清 VMCB Clean Bits（bit 5，L0 自己用，不转交语义）之外无妨的位**；清 **DecodeAssists（bit 7）若 L0 合并未实现**、**Pause Filter（bit 10/12）**、**AVIC（bit 13）**、**VMSAVE virtualization（bit 15）**、**LBR virtualization（bit 1）**；
  - EBX `NASID`（可用 ASID 数）如实上报但 L0 从每 CPU 命名空间给 VMCB02 分配 ASID（见 6.4）。
- **hypervisor 叶 0x4000_0001（HypervisorFeatures）**：定义 bit 0 = `nested` 可用，供 driver 探测。

### 3.2 相关 MSR

| MSR | 现状 | `nested=on` |
|---|---|---|
| `EFER` 0xC000_0080 | SVME 对 guest 读隐藏、写过滤 | SVME **读反映 L1 意图值**（per-vCPU `NestedSvmState.efer_svme`），写经 VMRUN 前合法性检查接受；真实 EFER.SVME 恒 1 |
| `VM_CR` 0xC001_0114 | Emulate 读 0 | 读返回 **SVMDIS=0、LOCK=1**（表示 SVM 可用且已解锁），其余位 0；写忽略（KVM 只读它判断可用性） |
| `VM_HSAVE_PA` 0xC001_0117 | 未列（Deny） | Emulate：存 L1 写入的 host save-area GPA（L0 不用它，仅为让 KVM 的 VMSAVE/VMLOAD 路径自洽） |

掩码总原则同 Intel：**L0 合并不了的能力一律不报**。NP 必须保留（KVM-amd 创建 L2 的硬性前提）。

## 4. C2：EFER.SVME 透传

KVM-amd 初始化路径：`cpu_has_svm()`（查 CPUID SVM 位 + VM_CR.SVMDIS）→ 写 `EFER.SVME=1` → `VMRUN`。现状 L0 过滤掉 guest 写 SVME。`nested=on` 时：

- 真实 EFER 恒 SVME=1（L0 自身在 host mode，本就如此）；
- guest 写 EFER 命中 AreaSwap，`wrmsr_virt` 把目标值的 SVME 位存入 `NestedSvmState.efer_svme`，VMCB save.efer 的 SVME 仍恒 1（VMRUN 一致性要求），guest 读 EFER 时由 `rdmsr_virt` 用 `efer_svme` 回填该位；
- guest 清 SVME（KVM 卸载）→ `efer_svme=0`，清空 `NestedSvmState`。

## 5. C3：VMRUN 拦截与 VMCB12 访问

### 5.1 per-vCPU 状态

```rust
#[cfg(feature = "nested")]
pub struct NestedSvmState {
    pub efer_svme: bool,                 // guest 的 EFER.SVME 意图
    pub hsave_pa: u64,                   // guest 写的 VM_HSAVE_PA（GPA）
    pub in_l2: bool,                     // 当前是否在跑 L2（VMCB02 active）
    pub vmcb12_gpa: u64,                 // 正在跑的 VMCB12 的 GPA
    pub nnpts: BTreeMap<u64, NestedNpt>, // key = L1 nCR3（合成页表缓存）
    // L1 host 上下文在 VMRUN 时即由硬件 VMSAVE 到 hsave 区，反射时恢复
}
```

SVM 相对简单：L1 的 host 状态在 VMRUN 时由硬件保存，L0 无需像 Intel 那样手工保存 host 区。L0 在切 VMCB02 前先 `vmsave` 自己的 VMCB01 host 态、退出后 `vmload` 回来即可。

### 5.2 VMRUN 语义（exit_code = VMEXIT_VMRUN = 0x80）

现状 `vmcb_setup` 已 `set_intercept(SvmIntercept::VMRUN, true)`，exit 走 `_ => ENOSYS`（注入 fault）。`nested=on` 时拦截之：

1. 读 L1 RAX = VMCB12 的 GPA；经 L0 GPM 翻译到 HPA，映射访问 VMCB12；
2. 校验 `efer_svme`（未置 → #UD 语义）、VMCB12 对齐；
3. 按 C4 合并 VMCB12 + L0 强制位 → VMCB02；
4. `vmsave` VMCB01 → 切 `in_l2=true` → VMRUN VMCB02；
5. L2 #VMEXIT 回 L0（§7 分类反射）。

VMLOAD/VMSAVE（现状已拦截）在 `nested=on` 时对 L1 给的 VMCB 做字段子集的 load/store 仿真（FS/GS/TR/LDTR base、STAR 系列等），不涉及 L2 运行，较简单。STGI/CLGI 仿真为 per-vCPU 的全局中断屏蔽标志（L0 记录，不真正开关硬件 GIF）。

## 6. C4：VMRUN 合并（VMCB12 → VMCB02）

总规则与 Intel 同构：

```
VMCB02.intercept = VMCB12.intercept ∪ L0_MANDATORY
VMCB02.msrpm     = L0_policy_msrpm | L1_msrpm   (每 CPU 合并页)
VMCB02.iopm      = L0_policy_iopm  | L1_iopm
VMCB02.n_cr3     = 合成 nNPT 根（C5），绝不指 L1 的 nCR3
VMCB02.guest 态  = VMCB12 guest 态直通（L0 做 SVM 合法性检查）
VMCB02.asid      = L0 分配（见 6.4）
```

| 字段 | 合并规则 |
|---|---|
| `intercept_cr/dr/exceptions/vector3-5` | `L1 ∪ L0_MANDATORY`。L0_MANDATORY = 现状 VMCB01 的强制拦截（NMI/CPUID/SHUTDOWN/VMRUN/VMMCALL/VMLOAD/VMSAVE/STGI/CLGI/SKINIT/MSR_PROT/IOIO_PROT）。L1 要拦更多 L2 事件可自由叠加 |
| `np_enable` | 恒 1（L0 用 nNPT 跑 L2） |
| `nest_cr3` | 合成 nNPT 根（C5） |
| `msrpm_base_pa` | 指向 L0 每 CPU 合并 MSRPM，绝不指 L1 页 |
| `iopm_base_pa` | 同上 |
| `guest_asid` | L0 分配（6.4），L1 的 ASID 值仅作 nNPT 缓存 key |
| `tlb_control` | VMRUN/上下文切换时按需 flush（配合 nNPT 失效） |
| `tsc_offset` | L1 值 + L0 基线（现状 L0 基线为 0，直通 L1 值） |
| guest save 区 | VMCB12 直通；L0 校验 CR0/CR4/EFER 合法性，不合法 → VMEXIT(VMEXIT_INVALID) 反射 |
| `int_control/vector/state` | L1 直通（L2 的虚拟中断归 L1 管），但 V_INTR_MASKING 等受 L0 中断策略约束 |

### 6.4 ASID 处理

L0 不把 ASID 命名空间让给 L1：VMCB02 的 ASID 由 L0 从每 CPU 计数器分配（避开 VMCB01 自用的 ASID=1）。任何 L2 上下文切换 / nNPT 失效都伴随 `tlb_control=FlushAsid`，因此无陈旧翻译风险。

### 6.5 MSRPM/IOPM 合并

- **MSRPM**：每 CPU 一页合并位图 `merged = L0_policy_msrpm | L1_msrpm`。L1 每次改 VMCB12.msrpm_base 后重建合并页。
- **IOPM**：同样 OR 合并；L0 强制端口集 = 现有 PIO 策略（PM1a、APMC）+ 0xCF8–0xCFF，L2 命中直接由 L0 处置，不反射（I3）。

## 7. C5 nNPT + C6 退出反射

### C5：合成 nNPT（L1 nNPT ∘ L0 GPM）

L2 地址翻译两级串联：L2GPA --(L1 nNPT)--> L1GPA --(L0 GPM)--> HPA。硬件只走一棵树，L0 为每个 L1 nCR3 维护一棵合成 `NestedNpt`（复用 `NestedPageTable` 框架）。缺页驱动构建：

```
L2 #NPF (exit 到 L0)
  ├─ walk L1 nNPT: L2GPA → (L1GPA, L1权限)
  │     ├─ L1 nNPT 缺页/权限不符 → 反射 #NPF 给 L1
  │     └─ 成功 ↓
  ├─ 查 L0 GPM: L1GPA → (HPA, L0权限)
  │     ├─ GPM 洞：安全内存→空页；受保护 MMIO→all-ones sinkhole（I1）
  │     └─ 成功 ↓
  └─ 合成表安装 L2GPA → HPA，权限 = L1 ∩ L0；resume L2
```

权限取交，安全边界由 L0 单侧保证（I4）。L1 刷自己 nNPT（改 nCR3 或 TLB flush）→ 对应合成表整棵丢弃重建。与 Intel 影子 EPT 完全同构，`src/arch/x86_64/` 公共层可共享合成逻辑（仅页表类型不同）。

### C6：退出反射分类

| Exit 类别 | 处置 |
|---|---|
| #NPF（L2GPA） | **L0**（C5 合成表路径）；L1 nNPT 层 fault 才反射 |
| VMMCALL | **L0 先行**：RAX 命中 monitor `HyperCallCode` 编码 → L0 分发；否则（KVM hypercall）反射给 L1（I3） |
| IOIO 命中 L0 策略端口（0xCF8–0xCFF/PM1a/APMC） | **L0** 处置，不反射 |
| IOIO 仅命中 L1 位图 | 反射 |
| MSR 命中 L0 策略 | **L0** 按策略表 |
| MSR 仅命中 L1 位图 / CPUID / 异常 / SHUTDOWN | 反射 |
| VMRUN/VMLOAD/... (L2 中执行) | 反射（L1 对 L2 隐藏 SVM，注入 #UD） |
| 外部中断 / NMI | **L0**（沿用现有中断直通与 enclave AEX 逻辑） |

**反射动作** `reflect_to_l1`：把 exit 信息写入 VMCB12（exit_code/info_1/info_2/exit_int_info），把 L2 guest 态写回 VMCB12 guest 区，`vmload` VMCB01 恢复 L1 host 态，`in_l2=false`，resume L1——对 KVM 而言与硬件直接把 L2 exit 给它无差别。

**互斥约束**：L2 运行与 EnclaveRunning 互斥（per-CPU 状态机单态）。enclave 活跃的 CPU 上 L1 的 VMRUN 返回错误；L2 运行中收到 enclave hypercall 按现有状态校验拒绝。

## 8. 验证策略

| 层 | 内容 | 工具 |
|---|---|---|
| 单元（宿主机 `cargo test`） | ① C4 拦截位/位图 OR 合并纯函数：L0_MANDATORY ⊆ merged；② C5 合成逻辑：fake L1 nNPT × fake GPM 权限取交、洞→sinkhole；③ VM_CR/EFER.SVME 仿真值；④ VMCB12 字段校验状态机 | `cargo test` |
| 构建门禁 | `make test`/`clippy`/`elf` 双厂商全绿；**`nested=off` 路径逐位等价** | 现有 Makefile |
| VM 内 bring-up（本阶段目标） | 在本机 KVM 的嵌套 VM（L1=Linux+KVM-amd）里跑 monitor，`NESTED=on` → L1 加载 KVM → 创建 L2 → L2 内 lscpu/内存压测 | QEMU/KVM 嵌套 |
| 真机 bring-up（最后） | AMD/Hygon 真机 + hyperenclave-driver | 交付后硬件任务 |

## 9. 里程碑切分

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **M1** | feature 骨架 + C1 能力暴露（CPUID SVM/0x8000_000A 掩码、VM_CR/EFER.SVME/VM_HSAVE_PA MSR）+ `NestedSvmState` | `nested=on` 构建双厂零 error；MSR/CPUID 仿真单测；KVM-amd 在 L1 能探测到 SVM 可用 |
| M2 | C3 VMRUN/VMLOAD/VMSAVE/STGI/CLGI 拦截 + VMCB12 访问 + 状态机 | KVM 能走到 VMRUN 仿真入口；VMCB12 读写单测 |
| M3 | C4 合并 + C5 合成 nNPT | L2 启动到内核；权限取交/sinkhole 合成单测 |
| M4 | C6 退出反射 + 位图合并 | L2 跑稳负载；L0 策略端口在 L2 内仍被拒 |
| M5 | VM 内 bring-up + 真机 | §8 矩阵全过 |

## 10. 限制清单（初版）

1. 每 CPU 单个 L2（`vmcb12_gpa` 唯一）；多 L2 缓存列优化项。
2. 无虚拟 IOMMU（I2），L2 设备直通走 L0 device hypercall 代理，不走 L1 VFIO。
3. nNPT 失效走整棵丢弃重建；细粒度失效列优化。
4. 无 AVIC / LBR virt / Pause Filter / VMSAVE virt（能力未报，置位即校验失败）。
5. L2 与 enclave 互斥。
6. Intel 侧为独立设计（nested-vmx-design.md），两侧共用公共合成框架与 feature 门控。
