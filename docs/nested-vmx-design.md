# 嵌套虚拟化（Nested VMX）实现设计

> **文档定位**：本文档是 G15（嵌套虚拟化，宿主系统正常管理自己创建的 VM）的实现级设计，关闭 [rustmonitor-v2-architecture.md §1.5](rustmonitor-v2-architecture.md) 与 [rustmonitor-v2-implementation-design.md §0.2](rustmonitor-v2-implementation-design.md) 登记的差距。目标形态即 HyperGPU 的 L0/L1/L2 三层（[rustmonitor-architecture.md §6.5.1](rustmonitor-architecture.md)）：L0 = 本 monitor，L1 = 降级后的 Host Linux（跑 KVM），L2 = L1 的 KVM 创建的 VM。
>
> **范围**：仅 Intel VMX；AMD SVM 本期不动（SVM 位对 guest 保持隐藏）。本文是**设计稿**——代码尚未实现；实现位于 cargo feature `nested` + Makefile `NESTED=on` 之后，**默认关闭**，关闭时行为与现状逐位一致（VMX 在 CPUID 中隐藏，VMX 指令 #UD）。
>
> **结构总览**：C1 能力暴露 → C2 CR4.VMXE 透传 → C3 VMCS12 仿真 → C4 VM-entry 合并 → C5 影子 EPT → C6 退出反射。前三个里程碑让 L1 的 KVM 完成模块加载与 VM 创建，后三个让 L2 真正跑起来。

---

## 0. 为什么默认关闭，以及"关闭 = 现状"

`nested=off` 时必须与当前行为**逐位一致**，这是回归底线：

- CPUID leaf 1 的 VMX 位继续清除（[cpuid.rs](../src/arch/x86_64/cpuid.rs) `apply_masks` 的既有行为）；
- VMX 能力 MSR 区 0x480–0x4BF 继续"读仿真为 0、写拒绝"（[msr/policy.rs:212-218](../src/arch/x86_64/msr/policy.rs#L212)）；
- L1 执行 VMXON 等指令：CR4 read shadow 中 VMXE=0 → #UD，与裸机上"无 VMX"语义一致。

`nested=on` 时打开 §3–§8 的全部机制。feature 分两个维度门控：编译期（`#[cfg(feature = "nested")]`，不编译即零成本零风险）+ 运行期状态机（per-vCPU `NestedVirtState`，未 VMXON 前一切如旧）。

## 1. 威胁模型与安全不变量

L1（降级 Linux 及其 KVM）**仍然完全不可信**——嵌套只是让它多了一层可以管理的执行域，不改变信任边界。安全不变量（每条都对应后面某节的机制）：

| # | 不变量 | 机制 |
|---|---|---|
| I1 | **受保护加速卡对 L1 和 L2 同样不可见**。设备隔离（[device-isolation-design.md](device-isolation-design.md)）挖在 L0 的 GPM 里；影子 EPT 的合成（C5）永远终结于 L0 GPM 查询，受保护 MMIO 解析到 all-ones sinkhole 页——L2 读到的也是全 1 | C5 |
| I2 | **真实 IOMMU 表只有 L0 编程**。不向 L1 暴露虚拟 VT-d（CPUID/MSR/ACPI 层面都不造），因此 **L2 设备直通不支持**——L1 的 KVM 给 L2 挂 VFIO 设备会在 L1 侧失败，这是设计内行为 | C1 |
| I3 | **L0 的强制拦截不可被 L1 撤销**：VMCALL hypercall 通路、MSR/PIO 策略端口（含 0xCF8/0xCFC 配置端口、PM1a、APMC）、监控策略位图。VMCS02 的位图由 L0 位图 **OR** L1 位图合成（C4），L1 只能增加自己的拦截，永远减不掉 L0 的 | C4 |
| I4 | L2 的内存访问必过合成 EPT（L1 EPT ∘ L0 GPM），L1 无法借 L2 触达安全内存/EPC | C5 |
| I5 | L0 对 L1 的既有约束原样保留：S3 拒绝（PM1a #GP）、SMM 锁定、LA57 fail-fast、CR0/CR4 保留位校验 | 既有机制 |

不防御：侧信道、DoS、硬件攻击（与全局威胁模型一致）。L1 的 KVM 崩溃/恶意最多毁掉 L2，伤不到 L0、Enclave 与受保护设备。

## 2. 总体结构：VMCS01 / VMCS12 / VMCS02

```
                 ┌──────────────────────────── L0 (monitor, VMX root)
                 │  VMCS01：跑 L1 的真实 VMCS（现状，L0 直接管理）
                 │  VMCS02：跑 L2 的真实 VMCS（每 CPU 一个，L0 合并生成）
                 │  影子 EPT：L1 EPT ∘ L0 GPM 的合成表（每 EPTP 一棵）
                 └──────▲──────────────▲──────────────────┘
              VM exit  │              │  VMLAUNCH/VMRESUME（L0 执行）
   ┌───────────────────┴───┐   ┌──────┴───────────────────┐
   │  L1 (Linux+KVM)        │   │  L2 (KVM 的 guest)        │
   │  VMCS12：KVM 为 L2     │   │  不知道自己嵌套；          │
   │  准备的 guest 侧 VMCS   │   │  看到的硬件 = VMCS02 合成  │
   │  （普通内存页，L0 仿真） │   │  出的世界                  │
   └────────────────────────┘   └──────────────────────────┘
```

命名约定（沿用 KVM nested 与 SDM 的通行叫法）：

- **VMCS01**：L0 跑 L1 用的真实 VMCS——就是现有代码里那个 VMCS；
- **VMCS12**：L1（KVM）为 L2 准备的 VMCS。对 L1 它是"硬件结构"，实际只是 L1 地址空间里一页普通内存，由 L0 按 C3 的语义仿真读写；硬件从不直接消费它；
- **VMCS02**：L0 为跑 L2 准备的真实 VMCS，每 CPU 一份，由 C4 的合并规则从 VMCS12 + L0 强制位生成。

## 3. C1：能力暴露（CPUID + VMX 能力 MSR）

### 3.1 CPUID

- **leaf 1 ECX[5] VMX**：`nested=on` 时保留真实值（1）；`off` 时维持清除。这是 `apply_masks` 中唯一随 feature 翻转的位。
- **hypervisor 叶 0x4000_0000**：沿用现有 "HyperEnclave" 签名；**0x4000_0001（HypervisorFeatures，现返回全 0）定义 bit 0 = `nested` 可用**，供 driver/工具探测，避免 KVM 试探性 VMXON 撞 #UD。
- leaf 0x8000_0001 的 SVM 位继续清除（AMD 不在范围内）。

### 3.2 VMX 能力 MSR：读 = 真实值掩码，写保持拒绝

MSR 策略表中 0x480–0x4BF 区段在 `nested=on` 时读动作从"Emulate 返 0"改为"**读真实 MSR 值并按 L0 可虚拟化集合掩码**"；**写动作维持 Deny 不变**（能力 MSR 本就只读，KVM 也不会写）。掩码总原则：**L0 仿真不了的能力一律不报**——L1 的 KVM 只会用它读到的能力，少报即安全。

| MSR | 名称 | 掩码规则 |
|---|---|---|
| 0x480 | IA32_VMX_BASIC | 真实值，**清 bit 55（VMCS shadowing）**；bit 48（VMCS 物理地址宽度限定 32 位）按真实值上报 |
| 0x481 | IA32_VMX_PINBASED_CTLS | 真实值 ∧ PIN_VIRTUALIZABLE：保留 external-interrupt exiting、NMI exiting、virtual-NMIs；**清 VMX-preemption timer**（初版不仿真，L1 要了也不给） |
| 0x482 | IA32_VMX_PROCBASED_CTLS | 真实值 ∧ PROC_VIRTUALIZABLE：保留 HLT/INVLPG/MWAIT/RDPMC/RDTSC exiting、CR3-load/store exiting、CR8-load/store、use MSR bitmaps、use I/O bitmaps、monitor trap flag、**activate secondary controls**；**清 interrupt-window/NMI-window exiting 以外未接线的位**，清 "use TSC offsetting" 视实现里程碑而定（C4 合并已支持则保留） |
| 0x483 | IA32_VMX_EXIT_CTLS | 真实值 ∧ EXIT_VIRTUALIZABLE：保留 host-address-space-size、acknowledge-interrupt-on-exit、save/load IA32_PAT、save/load IA32_EFER；**清 save/load IA32_PERF_GLOBAL_CTRL、save/load IA32_BNDCFGS、clear IA32_RTIT_CTL**（初版不合成 MSR load/store 列表，见 C4.6） |
| 0x484 | IA32_VMX_ENTRY_CTLS | 真实值 ∧ ENTRY_VIRTUALIZABLE：保留 IA-32e mode guest、load IA32_PAT、load IA32_EFER；**清 load IA32_PERF_GLOBAL_CTRL / BNDCFGS / RTIT**（同上） |
| 0x485 | IA32_VMX_MISC | 真实值，掩掉 activity-state HLT 之外的等待语义中与实现不符的位；CR3-target 数量如实上报（合并直通即可） |
| 0x486/0x487 | IA32_VMX_CR0_FIXED0/1 | 真实值直通（真实 CR0 约束对 L1 的 L2 maxime 同样成立，无需放松） |
| 0x488/0x489 | IA32_VMX_CR4_FIXED0/1 | 真实值（含 VMXE 在 FIXED0 中为 1——真实 CR4 恒 VMXE=1，见 C2，天然一致） |
| 0x48A | IA32_VMX_VMCS_ENUM | 真实值（L0 仿真按全字段表，不借高位裁剪） |
| 0x48B | IA32_VMX_PROCBASED_CTLS2 | 真实值 ∧ SECONDARY_VIRTUALIZABLE：保留 **EPT、VPID**、unrestricted guest（来自 primary 的 bit 7 对应位）、RDTSCP、descriptor-table exiting、XSAVES；**清 VMCS shadowing（bit 14）、VMFUNC（EPTP switching）、virtual-interrupt delivery / posted interrupts / APIC-register virtualization（APICv 全家）、ENCLS exiting、PML** |
| 0x48C | IA32_VMX_EPT_VPID_CAP | 真实值（EPT/VPID 是必须保留的能力，如实报） |
| 0x48D–0x491 | IA32_VMX_TRUE_*_CTLS | 与 0x481–0x484 同规则再掩一遍（KVM 优先读 TRUE 系列） |

要点：**unrestricted guest、EPT、VPID 必须保留**（KVM 创建 L2 的硬性前提）；VMCS shadowing、APICv、VMFUNC 明确不报——少报的能力 KVM 会优雅降级（慢但正确），多报一个仿真不了的能力就是硬伤。

## 4. C2：CR4.VMXE 透传

L1 的 KVM 初始化路径是 `cpu_has_vmx() → CR4.VMXE=1 → VMXON`。现状下 L0 的 CR4 guest/host mask 把 VMXE 钉死为 0（read shadow=0），L1 写 CR4.VMXE 触发 CR-access exit 后被拒。`nested=on` 时改为：

- **真实 CR4 恒保持 VMXE=1**（L0 自身在 root 模式，这一位本来就不允许清）；
- `CR4_GUEST_HOST_MASK` 继续覆盖 VMXE（写仍 exit 到 L0），但 **`CR4_READ_SHADOW` 的 VMXE 位改为镜像 L1 的意图值**：L1 写 CR4 的 exit handler 把 L1 目标值中的 VMXE 存入 per-vCPU `NestedVirtState.cr4_vmxe`，read shadow 随之更新；
- L1 清 VMXE（KVM 卸载）→ read shadow 归零、清空 `NestedVirtState`（等价 VMXOFF 语义）。

效果：L1 看到的 CR4.VMXE 完全是"自己的"，真实硬件从未变化。

## 5. C3：VMCS12 仿真

### 5.1 per-vCPU 状态

```rust
pub struct NestedVirtState {
    pub vmxon: bool,                    // VMXON 已执行
    pub current_vmcs: Option<GuestPhysAddr>, // VMPTRLD 选中的 VMCS12（GPA）
    pub launched: bool,                 // 该 VMCS12 已 VMLAUNCH 过（VMLAUNCH/VMRESUME 分野）
    pub shadow: AlignedPage,            // VMCS12 的 L0 侧缓存页（guest 内存的镜像）
    pub l1_host_state: L1HostState,     // C4 合并时保存的 L1 host 区（C6 反射用）
    pub shadow_epts: BTreeMap<u64, ShadowEpt>, // C5：key = L1 EPTP
    // …位图合并缓存等，见 C4.5
}
```

`shadow` 是 VMCS12 guest 页的内存镜像：VMREAD/VMWRITE 直接读写镜像，VMPTRLD/VMCLEAR 时与 guest 内存同步。镜像存在的理由：VMREAD/VMWRITE 极高频（KVM 每字段一次），每次都经 GPM 双翻译读 guest 内存太贵；且镜像让字段表查找成为纯内存操作。

### 5.2 字段编码表（SDM Vol.3 Appendix B）

VMCS 字段编码 = `index[9:0] | type[10] | width[12:11] | access[13]`（高位保留必须为 0，否则 VMfail）。VMCS12 内部偏移推导：

```
offset = index * 2 + (access == high ? 1 : 0)   // 单位：字段宽度的存储槽
width: 0=16-bit, 1=64-bit, 2=32-bit, 3=natural-width
```

字段在 4KB VMCS 内的布局按宽度分区：16-bit 区 0x0000–0x07FF、64-bit 区 0x0800–0x0FFF、32-bit 区 0x1000–0x17FF、natural-width 区 0x1800–0x1FFF。实现为一张静态字段表 `{ encoding, width, offset, 是否只读(read-only 区如 exit reason/qualification，VMWRITE 须 VMfail) }`，VMREAD/VMWRITE 经表翻译后对 `shadow` 做普通内存读写。**字段表是纯数据 + 纯函数，是宿主机单测的头号对象**（§9）。

### 5.3 指令语义

VMX 指令在 non-root 下**无条件 VM exit**（CR4.VMXE shadow=1 时；否则 #UD 不 exit）——exit reason 分别为 VMCLEAR=19、VMLAUNCH=20、VMPTRLD=21、VMPTRST=22、VMREAD=23、VMRESUME=24、VMWRITE=25、VMXOFF=26、VMXON=27、INVEPT=49、INVVPID=52。Intel 侧在 `intel/vmexit.rs` 的 dispatch 增加这些分支（现状未识别 reason 走 `ENOSYS` → 注入 fault 的路径，`nested=on` 时拦截之）。失败语义严格按 SDM：VMfailInvalid（CF=1，无 current VMCS）/ VMfailValid（ZF=1，错误号写入 VMCS12 的 VM-instruction error 字段）。

| 指令 | L0 仿真语义 |
|---|---|
| VMXON | 校验 CR4.VMXE shadow=1、操作数为 4KB 对齐 GPA；置 `vmxon=true`，记录 VMXON 区 GPA |
| VMXOFF | 要求 `vmxon && current_vmcs.is_none()`（有活跃 VMCS 时 VMfail）；清 `NestedVirtState`，CR4 shadow VMXE→0 |
| VMPTRLD | 读 GPA 处页的 revision id（低 31 位），与 IA32_VMX_BASIC 的 VMCS revision 比对（不匹配 → VMfailValid，错误 5=VMPTRLD with incorrect VMCS revision id）；将 guest 页内容载入 `shadow`，`current_vmcs = gpa`，`launched=false` |
| VMPTRST | 把 `current_vmcs` 的 GPA 写到 guest 指定内存操作数；无 current → VMfailInvalid |
| VMCLEAR | 把 `shadow` 写回 guest 页；清其 launch 状态；若清的是 current，则 `current_vmcs=None` |
| VMREAD | 经 §5.2 字段表从 `shadow` 读，写入 guest 寄存器/内存操作数；只读字段可读 |
| VMWRITE | 经字段表写 `shadow`；**对 read-only 区字段 VMfailValid**；对控制字段做合法性预检（不合法 → VMfailValid 错误 7/12 等，按 SDM 对应项） |
| VMLAUNCH / VMRESUME | 见 C4：合并 VMCS12 → VMCS02，切 VMCS02 进 L2。`launched=false` 只许 VMLAUNCH，`true` 只许 VMRESUME，违反 → VMfailValid |
| INVEPT / INVVPID | C5：丢弃整棵影子 EPT（全失效，简单且正确） |

### 5.4 分发位置

Intel `handle_exit` 的 match 增加 11 个 VMX 指令分支，统一进 `nested::handle_vmx_instruction(cpu, exit_reason, qualification)`；AMD 侧不加（SVM 位隐藏，guest 执行 VMRUN 等走 #UD 直通既有行为）。嵌套 handler 返回的 RIP 推进按各指令长度（exit instruction length 字段）处理，与现有 `advance_rip` 约定一致。

## 6. C4：VM-entry 合并（VMCS12 → VMCS02）

VMLAUNCH/VMRESUME 时，L0 把 VMCS12 翻译成真实可执行的 VMCS02。总规则：

```
VMCS02.control = (VMCS12.control ∩ VIRTUALIZABLE) ∪ L0_MANDATORY
```

- `VIRTUALIZABLE` = §3.2 能力掩码里报给 L1 的集合（L1 写了自己没被告知的位 → VMLAUNCH 时 VMfailValid，错误 12=VM-entry with invalid executive-VMCS 语义类比，报 VM-entry control 字段错误）；
- `L0_MANDATORY` = L0 运行 L1 的既有拦截面（VMCS01 上的强制位），**不可被 L1 移除**。

### 6.1 逐控制字段合并表

| VMCS 字段 | 合并规则 |
|---|---|
| Pin-based controls | `(L1 ∩ PIN_VIRTUALIZABLE) ∪ {L0 当期强制位}`。常态下 L0 无强制 pin 位；**enclave 运行态叠加的 external-interrupt exiting 等位属于 VMCS01，不进 VMCS02**（L2 与 enclave 互斥，见 §8） |
| Primary proc-based controls | `(L1 ∩ PROC_VIRTUALIZABLE) ∪ { USE_MSR_BITMAPS, USE_IO_BITMAPS, SECONDARY_CONTROLS, HLT(沿用 VMCS01 策略) }`。`UNCOND_IO_EXITING` 恒 0（与现状一致，性能底线） |
| Secondary proc-based controls | `(L1 ∩ SECONDARY_VIRTUALIZABLE) ∪ { ENABLE_EPT }`。ENABLE_VPID 若 L1 请求则置位，但 **VPID 字段由 L0 命名空间分配**（见 6.4） |
| VM-exit controls | **不取 L1 值**：VMCS02 的 exit controls 恒为 L0 集合（`VMEXIT_CTRL_MIN`：HOST_ADDR_SPACE_SIZE + SAVE/LOAD_PAT + SAVE/LOAD_EFER）。**L1 的 exit controls 记入 `NestedVirtState.l1_host_state`，在 C6 反射回 L1 时由 L0 手工应用**——硬件只把 L2 exit 给 L0，L1 想要的 exit 效果由 L0 在切换 VMCS02→VMCS01 时兑现 |
| VM-entry controls | `(L1 ∩ ENTRY_VIRTUALIZABLE)`；entry MSR-load 位初版屏蔽（capability 未报），L1 置位即 VMfailValid |
| Exception bitmap / page-fault 匹配 | L1 值直通（L2 的异常归 L1 管）；**L0 自身需要的异常位（enclave 路径）不叠加**——L2 运行与 enclave 互斥 |
| CR0/CR4 guest-host mask & read shadow | 见 6.3 |
| MSR bitmap 指针 | 指向 L0 合并页（6.5），**绝不指 L1 的页** |
| I/O bitmap A/B 指针 | 同上 |
| EPTP | 指向影子 EPT 根（C5），**绝不指 L1 的 EPT** |
| Guest 区（段/RS/RIP/…） | VMCS12 直通（L2 的状态归 L1 定义），L0 只做 SDM 合法性检查（检查结果不符 → VMfailValid） |
| Host 区 | **全部取 L0 自己的 host 值**（VMCS02 的 host 区 ≡ VMCS01 的 host 区）——L2 exit 回到的永远是 L0；L1 的 host 区保存在 `NestedVirtState` |

### 6.2 L1 host 区保存

VMLAUNCH 合并时，把 VMCS12 的 host 区（CR3/CR4、RIP/RSP、各段基址与选择子、MSR 列表）完整拷入 `NestedVirtState.l1_host_state`。C6 反射时用这份快照重建 L1 的执行上下文。这是"VMCS02 的 host 区恒为 L0"的配套——L1 的 host 语义由软件兑现。

### 6.3 CR0/CR4 mask/shadow 合并

```
merged_mask   = L0_mask | L1_mask
merged_shadow = (L1_shadow & ~L0_mask) | (L0_shadow & L0_mask)
```

L0 拥有的位（VMXE 等，见 C2）永远以 L0 shadow 为准；L1 新增的拥有位（如它想拦 L2 的 CR0.CD/NW）按 L1 shadow 呈现。L2 写 CR0/CR4 命中 `merged_mask` 即 exit 到 L0，L0 再按"L0 位自己仲裁、L1 位反射"的规则分流（C6）。

### 6.4 VPID 处理

L0 不把 VPID 命名空间让给 L1：VMCS02 的 VPID 字段由 L0 从每 CPU 计数器分配（避开 VPID 0 与 VMCS01 自用值），L1 写的 VPID 值仅作区分不同 L2 上下文的 key。任何 L2 上下文切换 / INVVPID / INVEPT 都伴随影子 EPT 失效（C5），因此 VPID 复用无陈旧翻译风险——**正确性靠"影子表整棵失效"，VPID 只是加速器**。

### 6.5 MSR/I/O bitmap 合并

- **MSR bitmap**：每 CPU 一页合并位图 `merged = L0_policy_bitmap | L1_bitmap`。L0 侧由既有策略表生成（[msr/policy.rs](../src/arch/x86_64/msr/policy.rs)，0x480–0x4BF 区在 `nested=on` 时读位仍拦截——读已改为掩码仿真而非直通）。L1 每次 VMWRITE 改 bitmap 指针后重建合并页（读 L1 页 + OR + 写合并页）。
- **I/O bitmap**：同样 OR 合并。L0 侧的强制端口集合 = 现有 PIO 策略表（PM1a、APMC）**加 0xCF8–0xCFF 配置端口**——即"宿主降级后运行边界"里 L0 必须仲裁的全部端口，L2 命中这些端口由 L0 直接处置，不反射（I3）。

### 6.6 初版明确不做

L1 的 VM-entry/exit MSR load/store 列表（capability 中对应位已掩掉，L1 置位即 VMfailValid）；L1 的 LBR/PT/BND 类调试追踪控制（与全局 fail-closed 一致）；VMCS shadowing 与 eVMCS（见 §10 限制清单与后续工作）。

## 7. C5：影子 EPT（L1 EPT ∘ L0 GPM 合成）

L2 的地址翻译是两级串联：L2GPA --(L1 的 EPT)--> L1GPA --(L0 的 GPM)--> HPA。硬件只能走一棵树，因此 L0 为每个 L1 EPTP 维护一棵**影子 EPT**（`MemorySet<NestedPageTable>`，复用现有页表框架），内容 = 两级的合成结果。

**缺页驱动构建**（影子表惰性填充）：

```
L2 EPT violation (exit 到 L0)
  ├─ walk L1 EPT: L2GPA → (L1GPA, L1权限)
  │     ├─ L1 EPT 本身缺页/权限不符 → 反射 EPT violation 给 L1（C6）
  │     └─ 成功 ↓
  ├─ 查 L0 GPM: L1GPA → (HPA, L0权限)
  │     ├─ GPM 洞（安全内存/受保护设备 MMIO）→ 按 GPM 语义处理：
  │     │     安全内存 → 空页；受保护 MMIO → all-ones sinkhole 页（I1 成立于此）
  │     └─ 成功 ↓
  └─ 影子表安装 L2GPA → HPA，权限 = L1权限 ∩ L0权限；resume L2
```

- **权限取交**：L1 想给 L2 的写权限越不过 L0 GPM 的只读/空洞——安全边界由 L0 单侧保证，不依赖 L1 EPT 的正确性（I4）。
- **INVEPT/INVVPID 处理**：L1 执行即整棵影子表丢弃（重建交给后续缺页）。简单、必然正确；细粒度失效（按 GPA 区间）列后续优化——正确性优先，性能靠 EPT 大页与惰性重建兜底。
- **反射条件**：walk L1 EPT 时发现 L1 层面的 fault（not-present / 权限 / misconfig），合成为 VMCS12 的 exit qualification 后反射给 L1（KVM 的 L2 MMU 会处理自己的缺页）。

## 8. C6：退出反射分类表

L2 运行期间所有 exit 先到 L0。分类处置：

| Exit 类别 | 处置 | 说明 |
|---|---|---|
| EPT violation（L2GPA） | **L0**（C5 影子表路径）；L1 EPT 层 fault 才**反射** | 影子表维护是 L0 职责 |
| VMCALL | **L0 先行**：rax 命中 monitor `HyperCallCode` 编码（Supervisor < 0x4000_0000 / User ≥ 0x8000_0000）→ L0 hypercall 分发；否则（KVM hypercall ABI）**反射**给 L1 | monitor hypercall 通路不可被 L1 截获（I3） |
| PIO 命中 L0 策略端口（0xCF8–0xCFF、PM1a、APMC） | **L0** 按策略处置（全 1 / #GP / 丢弃），不反射 | I3；L2 同样看不到受保护设备 |
| PIO 仅命中 L1 位图 | **反射** | L1 的虚拟设备语义 |
| MSR 命中 L0 策略位图 | **L0** 按策略表（Deny→#GP 注入 L2 视角等价于反射后 L1 注 #GP，但 0x480 区等由 L0 直接仿真掩码值） | L1 读 VMX 能力 MSR 时得到的本就是 L0 掩码后的值，其 KVM 为 L2 准备的数值天然被约束——反射给 L1 让 KVM 自己仿真也安全 |
| MSR 仅命中 L1 位图 / CPUID / 异常 bitmap 命中项 / triple fault 等 | **反射** | L1 的 CPUID 仿真基于它自己被 L0 掩码过的视图，结果对 L2 天然合规 |
| VMX 指令（L2 中执行） | **反射** | L1 的 KVM 会对 L2 隐藏 VMX，注入 #UD——L0 不代劳 |
| 外部中断 / NMI | **L0**（沿用现有中断直通与 enclave AEX 逻辑） | 中断路由权属 L0 现状机制 |

**反射动作**（统一函数 `reflect_to_l1(cpu, exit_ctx)`）：

1. 把 exit 信息写入 VMCS12（`shadow`）的 read-only 区：exit reason、exit qualification、exit instruction info/length、IDT-vectoring 信息、guest-linear address；
2. 用 `NestedVirtState.l1_host_state`（C4.2 保存）把 L1 host 上下文装入 VMCS01 的 guest 区（RIP=RIP_l1_exit_handler 等），同时把 L2 的 guest 状态写回 VMCS12 guest 区；
3. `vmptrld VMCS01` → `vmresume`——回到的是 L1 的 KVM exit handler，对它而言与硬件直接把 L2 exit 给它无差别。

L1 再次 VMLAUNCH/VMRESUME 时走 C4 重新合并（VMCS12 可能已被 KVM 改）。`launched` 标志随反射置位。

**互斥约束**：L2 运行与 EnclaveRunning 互斥（per-CPU 状态机单态）。enclave 活跃的 CPU 上 L1 发起的 VMLAUNCH 返回 VMfailValid；反之 L2 运行中收到 enclave 相关 hypercall 按现有状态校验拒绝。

## 9. 验证策略

| 层 | 内容 | 工具 |
|---|---|---|
| 单元（宿主机 `cargo test`） | ① §5.2 字段表：编码→offset/宽度/只读性与 SDM 附录 B 样例逐字段对拍；② C4 控制合并纯函数：∀ 输入，L0_MANDATORY ⊆ merged 且 merged ⊆ (L1 ∪ L0_MANDATORY)，位图 OR 合并单调性；③ C5 合成逻辑：fake L1 EPT × fake GPM 的权限取交、洞映射到 sinkhole；④ C3 指令语义状态机（VMfailInvalid/Valid 分野、launch/resume 配对） | `cargo test`（no_std 可测部分全走宿主机） |
| 构建门禁 | `make test` / `make clippy` / `make elf` 双厂商（VENDOR=intel + VENDOR=amd）全绿；**AMD 构建必须零影响**（feature off 路径逐位等价） | 现有 Makefile |
| 默认关闭回归 | `nested=off` 构建下跑既有单元测试与 CI 冒烟，证明 VMX 隐藏等现状行为不变 | 现有门禁 |
| **实机 bring-up（必需后续项）** | Intel VMX 物理机 + hyperenclave-driver + Linux 5.10：`nested=on` 激活 → L1 加载 KVM → 创建 L2 → L2 内跑通 lscpu/内存压测 → 关 KVM 模块验证 VMXOFF 路径；设备隔离回归（L1/L2 均读不到受保护卡） | **本环境无法执行，必须列为交付后的实机任务** |

明确声明：本文档交付物为设计 + 宿主机可验证的纯逻辑实现；**真机 KVM bring-up 不在本环境能力范围内**，是合入前的硬性后续门槛。

## 10. 限制清单（初版）

1. **每 CPU 单个 L2 VMCS**：`current_vmcs` 唯一，KVM 在同一 pCPU 上切换 L2 vCPU 走 VMCLEAR+VMPTRLD（正确，慢）；多 current VMCS 缓存列优化项。
2. **无虚拟 IOMMU**：不向 L1 暴露 VT-d 能力（I2），L2 设备直通不支持；L2 的 virtio 等半虚拟化路径不受影响。
3. **无 VMCS shadowing / eVMCS**：VMREAD/VMWRITE 全 exit 仿真。eVMCS 是 HyperGPU 材料中的性能优化方向，作为后续里程碑叠加在本设计的字段表之上（仿真语义不变，只换快速通路）。
4. **INVEPT/INVVPID 全量失效**：整棵影子 EPT 丢弃重建；细粒度失效列优化。
5. **AMD SVM 不动**：SVM 位对 guest 保持隐藏；nested SVM 是独立设计题。
6. **无 L3**：L2 中 VMX 不可见（L1 的 KVM 基于被掩码的 CPUID 视图自然不给 L2 VMX）。
7. **L1 MSR load/store 列表不支持**（capability 未报，置位即 VMfailValid）。
8. **L2 与 enclave 互斥**（per-CPU 状态机单态，§8）。

## 11. 里程碑切分建议

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M1 | feature 骨架 + C1 能力暴露 + C2 CR4.VMXE | KVM 模块在 L1 加载成功（`nested=on`），VMXON 走到仿真入口 |
| M2 | C3 VMCS12 仿真（字段表 + 指令状态机，不含 entry） | 字段表单测全绿；KVM 能创建/销毁 VM（VMPTRLD/VMCLEAR 往返） |
| M3 | C4 VM-entry 合并 + C5 影子 EPT | L2 启动到内核；权限取交与 sinkhole 合成单测 |
| M4 | C6 退出反射 + bitmap 合并 | L2 跑稳负载；L0 策略端口在 L2 内仍被拒 |
| M5 | 实机 bring-up + eVMCS 评估 | §9 实机矩阵全过 |
