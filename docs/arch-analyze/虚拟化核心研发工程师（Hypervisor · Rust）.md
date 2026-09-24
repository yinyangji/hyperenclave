# 虚拟化核心研发工程师（Hypervisor · Rust）

> **文档使用说明**：本文档分两部分。第一部分为对外 JD，可直接发布至招聘渠道；第二部分为面试题库与评分标准，仅供内部面试官使用，不对外发布。

---

## 第一部分 对外 JD

### 岗位名称

虚拟化核心研发工程师（跨架构 Hypervisor · Rust）

### 岗位简介

本岗位服务于基于 Rust 的裸机 hypervisor 安全监控器研发（开源项目 HyperEnclave 的演进版本）。监控器运行于 CPU 最高特权层（x86-64 Intel VT-x / AMD SVM root 模式，ARM64 EL2），在缺少硬件 TEE 的通用服务器上与外置硬件可信根（FPGA 类安全芯片）软硬协同，为上层机密计算提供可验证的隔离底座。虚拟化层需覆盖鲲鹏、飞腾（ARM64）与海光、AMD、Intel（x86-64）多平台。岗位工作以虚拟化核心子系统研发为主要方向：执行流控制与拦截、二级地址翻译（EPT/NPT/Stage-2）、敏感资源（MSR/CPUID/系统寄存器/IO）虚拟化、设备直通与 DMA 隔离（VT-d/AMD-Vi/SMMU）、跨架构抽象与多平台适配。

### 工作内容

- 设计与实现 MSR/系统寄存器虚拟化子系统：拦截策略表、x86 MSR bitmap 与 VMCB MSRPM 双厂商位图生成、VMCS MSR load/store area 硬件切换、ARM64 系统寄存器 trap 配置，按寄存器逐项定义仿真语义

- 建设 CPUID/ID 寄存器策略引擎：启动期快照、特性位掩码、拓扑枚举受控视图

- 维护与扩展多架构执行流控制：x86 VMCS/VMCB 拦截位图、异常注入、CR0/CR4 guest-host mask 与 read shadow；ARM64 EL2 配置（HCR_EL2 trap 位、Stage-2 翻译表、VHE 宿主内核运行）

- 实现二级地址翻译视图管理：EPT/NPT/Stage-2 页表、安全内存空映射、保护域双视图切换、大页映射策略

- 实现设备与 DMA 隔离：VT-d/AMD-Vi/SMMU 重映射配置、设备直通与归属仲裁、共享页与设备复位管理

- 对接外置硬件可信根：经 PCIe/DMA 受控队列与 FPGA 安全芯片交互，协同完成度量启动、远程证明与密钥放行

- 建设跨架构抽象层与多平台适配矩阵：在 x86（VMX/SVM）与 ARM64（EL2）间提炼统一虚拟化接口，针对鲲鹏/飞腾/海光/AMD/Intel 完成适配与验证

- 在 QEMU 嵌套虚拟化环境与多架构实机环境搭建并维护测试矩阵

### 任职要求

1. **计算机体系结构**：深入理解 x86-64 长模式分页（四级页表、TLB 语义与刷除时机）、段机制与特权级、LAPIC/x2APIC 中断体系、IDT 与异常错误码模型、EFER/PAT/MTRR/STAR/SYSENTER 族 MSR 语义；或深入理解 ARM64 异常级别（EL0–EL3）、Stage-1/Stage-2 地址翻译、VHE、GICv3/v4 中断体系、系统寄存器编码与 trap 机制。两者至少精通其一，并具备向另一架构迁移的能力

2. **硬件虚拟化执行模型**：精通 Intel VMX（VMCS guest/host/control 三区字段、vmlaunch/vmresume 状态机、instruction error 排障）、AMD SVM（VMCB save/control 区、vmrun/vmload/vmsave）或 ARM EL2 虚拟化（HCR_EL2 trap 控制、Stage-2 fault 处理、EL2 异常向量）至少一种，理解特权层切换时硬件自动保存与恢复的状态集合

3. **拦截机制配置**：具备 MSR bitmap、IO bitmap、异常位图、CR guest-host mask 与 read shadow、VM-entry/exit MSR load/store area（x86），或 HCR_EL2 trap 位、CPTR_EL2、Stage-2 权限控制（ARM）中至少三项的实际配置经验

4. **二级地址翻译**：掌握 EPT/NPT（x86）或 Stage-2（ARM）页表层级与大页映射、violation/fault 限定字段解析，理解 INVEPT/INVLPGA（x86）或 TLBI 指令（ARM）的适用时机

5. **Rust 裸机工程**：熟练进行 no_std 环境开发，掌握内联汇编与 naked function 编写，能够为 unsafe 抽象给出健全性论证

6. **规范精读能力**：能够按章节定位并精确解读 Intel SDM Vol.3C、AMD APM Vol.2 或 ARM Architecture Reference Manual（DDI 0487）的虚拟化相关章节，工程决策以架构规范为准绳

### 优先条件

- 具备国产 CPU 平台（鲲鹏、飞腾、海光）虚拟化适配或 bring-up 经验

- 具备 VT-d、AMD-Vi 或 ARM SMMU（SMMUv3）DMA 重映射配置实践

- 熟悉 GICv3/v4 虚拟化（vGIC、List Register、ITS）、APICv、posted interrupt、AVIC 任一中断虚拟化机制

- 研读过 KVM（arch/x86 或 arch/arm64）、hvpp、SimpleVisor、Jailhouse 等开源 hypervisor 实现源码

- 具备外置硬件可信根协同经验：FPGA/TPM 类安全芯片对接、度量启动、远程证明、密钥管理

- 参与过 TEE、SGX、TDX、SEV-SNP、ARM CCA 或机密计算方向项目

- 有 GPU/加速卡设备直通、PCIe 设备驱动或 DMA 引擎开发经验

- 有 QEMU 嵌套虚拟化调试经验，掌握 GDB remote 与串口日志排障方法

- 对 Coq 等形式化验证工具有基础

---

## 第二部分 内部面试材料（不对外发布）

### 一、简历筛选要点

**强信号（直接进入面试）**：

- 向 KVM/QEMU/Xen 等社区提交过虚拟化相关 patch（附 commit 链接）

- 独立编写或深度参与过 hypervisor 项目（课程实现、研究原型、开源项目均可，需能说明细节）

- 工作经历中含 VMM、IOMMU、设备直通、机密计算（TDX/SEV\-SNP）任一方向

- 技术博客或论文中引用 SDM/APM/ARM ARM 具体章节讨论过虚拟化细节

- 具备 ARM64 EL2 虚拟化（Stage-2、GICv3/v4、SMMU）或国产 CPU 平台（鲲鹏/飞腾/海光）适配经验

- 有外置硬件可信根（FPGA/TPM）软硬协同、度量启动或远程证明实现经验

**弱信号（需电话初筛确认）**：

- 仅使用过虚拟化产品（云平台运维、VirtualBox/KVM 使用）而无内核或 VMM 层开发

- 简历提及"虚拟化"但内容集中于容器、K8s（属不同领域）

- Rust 经验全部来自应用层开发，无 embedded/no\_std 背景

**电话初筛确认问题**（10 分钟，二选一）：

1. “VM-exit（或 ARM 的 EL2 trap）发生时，通用寄存器是硬件保存还是软件保存？”（合格：软件保存，通常由 exit/trap handler 入口汇编完成）

2. “guest 访问一个二级页表（EPT/Stage-2）未映射的地址会发生什么？”（合格：触发 VM-exit/Stage-2 fault 陷入最高特权层，处理器不产生 guest 可见的缺页）

两题均答错，终止流程。

### 二、面试题库

> 每题标注考察维度与分值。合格信号给出预期答案要点；警示信号给出典型错误。面试官按候选人实际回答对照评分，不要求逐题全问，每个维度至少一题。

#### 维度 A：x86\-64 体系结构（权重 12%）

**A1\.（2 分）描述 64 位模式下一次内存访问从线性地址到物理地址的完整翻译过程。PCID 在其中起什么作用？**

- 合格信号：四级页表逐级 walk（PML4→PDPT→PD→PT）；PCID 区分不同地址空间的 TLB 项，避免 CR3 切换全量刷除；能顺带说明 PGD/大页变体者佳

- 警示信号：只说"查页表"而无层级概念；混淆 PCID 与 ASID 的作用域（PCID 在普通分页，ASID 在 SVM/NPT）

**A2\.（2 分）SYSCALL 指令执行时发生了什么？涉及哪些 MSR？SYSRET 如何确定返回地址？**

- 合格信号：SYSCALL 从 STAR MSR 取 CS/SS（低 32 位内核、高 32 位用户选择子），RIP 从 LSTAR 取，RFLAGS 掩码取 SFMASK，返回地址存 RCX、标志存 R11；SYSRET 从 RCX/R11 恢复；能指出 FMASK 常见置位 IF

- 警示信号：认为 syscall 走 IDT（那是 int 指令路径）；说不出 STAR 与 LSTAR 的分工

**A3\.（1 分）\#GP 与 \#UD 在异常注入时有何差异？哪些向量带 error code？**

- 合格信号：注入时 VM\-entry interruption information 的类型字段不同；带 error code 的异常（\#GP/\#PF/\#SS 等）需额外写 VM\-entry exception error code 字段；\#UD 不带

- 警示信号：不了解注入时 error code 需要单独字段传递

**A4\.（2 分）x2APIC 模式下如何发送一个 IPI？与 legacy 模式的差异？**

- 合格信号：写 ICR MSR（0x830）即触发发送，非内存映射；legacy 走 MMIO ICR 寄存器且分高低 32 位两次写；能说明 ICR 写即动作、读回通常为 0 或固定值——这正是 MSR 仿真需要注意的语义

- 警示信号：不知道 ICR 写即触发发送，认为只是设置寄存器状态

#### 维度 B：VMX/SVM 执行模型（权重 20%）

**B1\.（2 分）VMLAUNCH 失败有哪些路径？如何定位失败原因？**

- 合格信号：区分 launch fail（VMCS 无 current VMCS，读 instruction error 得失败码）与 VM\-exit 后 guest 状态异常两类；排障用 VM\-instruction error 字段 \+ 逐项核对 control 字段的 must\-be\-1/must\-be\-0 约束（对照 IA32\_VMX\_\*\_CTLS）

- 警示信号：只回答"看日志"；不了解 fixed 控制位约束的检查方法

**B2\.（3 分）VM\-exit 发生时，硬件自动保存与恢复哪些状态？哪些状态必须由软件处理？**

- 合格信号：硬件保存 guest 通用状态区（RIP/RSP/RFLAGS/段/CR/DR 等）并加载 host 状态区（host RIP/RSP/CR/段）；通用寄存器 RAX–R15 不保存，由 exit handler 入口汇编保存；浮点/XSAVE 状态默认不切换；FSGSBASE 类 MSR 默认不切换（除非配置 load/store area）——此题直接关联本岗位 MSR 子系统设计

- 警示信号：认为硬件保存一切（无汇编入口的概念）；或认为通用寄存器也是 VMCS 字段

**B3\.（2 分）AMD VMCB 的 clean bits 机制解决什么问题？误用会有什么后果？**

- 合格信号：clean bits 声明 VMCB 哪些区域自上次 vmrun 以来未变，硬件可跳过重新加载以提速；误清（标记 dirty 但实际没改）仅损失性能，误置 clean（改了却标记 clean）会导致硬件使用过期状态——语义正确性 bug

- 警示信号：只答"性能优化"说不出错误方向的后果差异

**B4\.（2 分）VMCS 的 current/active 状态机如何运转？VMCLEAR 的必要性是什么？**

- 合格信号：vmptrld 指定 current VMCS；同一 VMCS 只能在一个逻辑处理器上 active；迁移或释放前必须 VMCLEAR（写回 shadow 状态、解除 active）； launch 状态（launched/clear）决定 vmlaunch 与 vmresume 的使用

- 警示信号：不了解 VMCLEAR 在多核场景的必要性

**B5\.（2 分）unrestricted guest 模式的价值是什么？为什么本项目可以依赖它？**

- 合格信号：允许 guest 在分页关闭、实模式下直接运行而无需仿真；本场景 Linux 降级时已是保护模式且分页开启，依赖它可简化 CR0\.PE/PG 的处理（guest 可自由切换）

- 警示信号：与 unrestricted guest 的适用前提混淆

#### 维度 C：拦截机制配置（权重 20%）

**C1\.（3 分）MSR bitmap 中某位从 0 改为 1，guest 的 RDMSR 与 WRMSR 分别发生什么变化？bitmap 的四个区域如何组织？**

- 合格信号：对应 MSR 的读（或写）从直接执行变为触发 VM exit；四区各 1KB：read\-low（MSR ≤ 0x1FFF）、read\-high（0xC000\_0000 起）、write\-low、write\-high；位寻址 = 区内按 MSR 偏移；能指出"位置 1 表示拦截"这一约定（AMD MSRPM 相反或不同，指出两厂商编码差异者佳——本项目 F1 案例：用 `&=` 置位导致全失效，位运算方向是常见坑）

- 警示信号：答不清四区布局；不知道置位语义；混淆 Intel bitmap 与 AMD MSRPM 的编码

**C2\.（3 分）CR4 的 guest\-host mask 与 read shadow 如何协作？guest 读 CR4、写 CR4 各返回/触发什么？**

- 合格信号：读：mask=0 的位返回 guest CR4 真值，mask=1 的位返回 shadow 值；写：mask=1 的位若新值 ≠ shadow 触发 exit（软件仲裁后改 shadow），mask=0 的位直接生效；典型用途：VMXE 位 host 独占（mask=1，shadow=0），guest 永远看不到也不改

- 警示信号：说"写 CR4 一律 exit"（不了解 mask 机制的本意是减少 exit）；或说不出 shadow 在读路径的参与

**C3\.（2 分）USE\_IO\_BITMAPS 与 UNCOND\_IO\_EXITING 的差异？为什么生产环境应避免后者？**

- 合格信号：前者按端口位图选择性拦截，后者无条件拦截全部 IO 指令；内核常规 IO 操作频繁（如端口 IO 的 ATA/老设备/ACM），全拦截产生海量 exit，性能不可接受；本项目策略是仅拦截电源管理与 SMI 触发相关端口（PM1x/APMC）

- 警示信号：认为 hypervisor 应该全部拦截才安全（无直通优先的性能意识）

**C4\.（2 分）VM\-exit MSR store area 的条目数上限如何获取？超出配置会怎样？**

- 合格信号：上限 = IA32\_VMX\_MISC\[25:8\] \+ 1（典型 256）；超出时 VM entry 失败（VM\-entry failure），属于配置错误必须在开发期断言

- 警示信号：不知道上限来自哪个 MSR

**C5\.（2 分）异常位图（EXCEPTION\_BITMAP）如何支持 enclave 的异步退出语义？**

- 合格信号：位图按向量号置位，命中向量的异常触发 exit 而非按 guest IDT 派发；本项目在 enclave 进入时全量置位异常位图（0xFFFF\_FFFF），使任何异常都上交 monitor 走 AEX/fixup 路径，退出时清零恢复直通——动态切换拦截面是本题考点

- 警示信号：只答"拦截异常"，说不出进入/退出时的动态切换用途

#### 维度 D：二级地址翻译（EPT/NPT）（权重 8%）

**D1\.（2 分）EPT violation 的 exit qualification 里有哪些关键字段？软件如何区分读/写/执行违规？**

- 合格信号：违规的 guest 物理地址、访问类型位（读/写/取指）、GVA 可选提供（enable EPT violations \#VE 或 GVA translation）、final translation 位；据此决定注入 guest \#PF、建立映射或走保护域逻辑

- 警示信号：不了解 exit qualification 携带访问类型

**D2\.（2 分）INVEPT 的两种类型（single\-context / all\-context）各适用什么场景？**

- 合格信号：修改了某 EPTP 的页表后可单上下文失效（按 EPTP），全局性变更（如重映射安全内存）用 all\-context；能对照 INVLPGA（SVM 按 ASID\+地址）者佳

- 警示信号：不知道为什么不能用普通 INVLPG（那是 guest 线性地址语义）

**D3\.（2 分）guest 写 CR3 应该拦截吗？KVM 与本项目为何选择不同？**

- 合格信号：KVM 拦截 CR3（影子页表时代遗留 \+ 分时复用需要跟踪切换 vCPU 上下文）；本项目 1:1 直通（每 vCPU 对应固定 pCPU，guest 页表自管，CR3 写无需仲裁，EPT 兜底物理边界）——考察对"拦截面 = 越权面"原则的理解深度

- 警示信号：认为必须拦截 CR3（未理解分区直通模型与二级翻译的兜底关系）

#### 维度 E：Rust 裸机工程（权重 12%）

**E1\.（3 分）no\_std 裸机环境下如何实现全局可变状态？static mut 直接引用有什么问题？**

- 合格信号：static mut 的引用是 UB 温床（别名规则，多核并发访问，Rust 2024 已收紧为硬错误方向）；正确方案：原子类型、spin::Mutex、或者本项目采用的 BSS 预留 \+ 一次性初始化封装（LateInit 模式：UnsafeCell\<MaybeUninit\> \+ AtomicBool 守卫，init 一次后 get 返回 \&'static T）；能论证 unsafe 边界的写法（不变量写进文档与断言）

- 警示信号：认为"裸机没有并发问题"；或直接给出 transmute 方案且不谈不变量

**E2\.（2 分）为什么 VMX exit 入口必须是 naked function？普通函数会有什么问题？**

- 合格信号：编译器会在普通函数入口插入 prologue（push rbp 等指令与栈帧建立），破坏精确的寄存器保存协议——exit 入口的每条指令都在操作 guest 寄存器现场；naked fn 保证函数体即裸汇编；能说明本仓库 vmx\_exit 的保存协议（save\_regs\_to\_stack 后切 host 栈再调用 handler）者佳

- 警示信号：不了解 prologue 的存在；答不出"精确控制指令序列"这一根本原因

**E3\.（3 分）读以下代码，指出问题并给出修复方向**（现场出示）：

```Rust
static mut EMPTY_BUFFER: [usize; 1024] = [0; 1024];
pub static MANAGER: &Manager = unsafe { core::mem::transmute(&EMPTY_BUFFER) };
```

- 合格信号：这是先占 BSS 后初始化的模式，但 transmute 直接把未初始化内存当合法引用——在 init 完成前任何访问都是 UB；修复：MaybeUninit \+ init 原子标记（LateInit），或改用 spin::Once/lazy\_static（若运行时允许）；进一步指出：即使是合法引用，\&Manager 携带内部可变性时多核并发仍需锁——能多角度论证者给予加分

- 警示信号：看不出问题（认为 transmute 数组到引用是安全惯用法）；或只说"加 unsafe 块就行"

#### 维度 F：安全思维与综合设计（权重 8%）

**F1\.（2 分）guest 访问一条策略表未定义的 MSR，monitor 应该返回什么？为什么？**

- 合格信号：默认拒绝（\#GP 或恒 0 \+ 记录日志），即 fail\-closed；理由：安全监控器无法预知未知 MSR 的副作用，放行的风险不对称——拒绝最坏是功能异常（可发现可修复），放行最坏是静默安全漏洞；能对比 v1 行为（读返 0 写丢弃的"静默丢弃"）为何是未定义行为者佳

- 警示信号：倾向默认放行（兼容性优先），且无审计日志意识

**F2\.（2 分）为什么 LBR（Last Branch Record）、Intel PT、PEBS 类调试设施必须对 guest 关闭？**

- 合格信号：这些设施可记录 enclave 内部控制流与分支/内存访问时序，构成机密性侧信道（监控器承诺 enclave 内容机密，但调试设施在 enclave 运行时仍在记录）；所以 DEBUGCTL/RTIT 系列 MSR 一律拒绝

- 警示信号：只答"性能问题"；不知道这些设施与 enclave 机密性的关系

**F3\.（3 分）现场设计题：需求是"guest 读取 MSR 0x35（CORE\_CAPABILITIES）时返回固定值 0，写入则注入 \#GP"。请给出完整实现方案，覆盖拦截配置、handler 逻辑、双厂商差异。**

- 合格信号（按要点累计）：

    1. 策略表加项 \{msr: 0x35, read: Emulate, write: Deny\}；

    2. Intel：MSR bitmap read\-low 区置位 0x35 的位（注意 \|= 而非 \&=）；

    3. AMD：MSRPM 中 0x35 对应的读/写两个 bit 位（按 APM 编码定位）置拦截；

    4. handler：read 路径返回 0 到 RAX/RDX 并 advance RIP 2 字节；write 路径注入 \#GP（带 VM\-entry interruption info）不 advance RIP；

    5. 加分：指出 AMD 与 Intel 的 exit reason 不同（MSR exit 0x4C / SVM MSR exit code）、注入 \#GP 不前进 RIP 与正常路径 advance RIP 的差异原因（\#GP 指向原指令）

- 警示信号：方案只写 handler 逻辑而完全遗漏位图配置（不知道拦截由硬件配置触发）；advance/rollback RIP 方向搞反

#### 维度 G：ARM64 EL2 与跨架构虚拟化（权重 12%）

**G1.（2 分）ARM 异常级别 EL0–EL3 各自的职责？EL2 在虚拟化中扮演什么角色？VHE 解决什么问题？**

- 合格信号：EL0 用户态、EL1 内核/OS、EL2 hypervisor、EL3 安全监控（ATF）；EL2 接管 Stage-2 翻译与 trap，运行宿主内核（VHE）或独立 hypervisor；VHE 让宿主内核直接跑在 EL2（HCR_EL2.E2H=1），避免传统 EL1/EL2 频繁切换开销——能对照 x86 VMX root/non-root 者佳

- 警示信号：混淆 EL2 与 EL3 职责；不知道 VHE 的动机

**G2.（3 分）Stage-2 翻译（IPA→PA）与 Stage-1（VA→IPA）如何协作？Stage-2 translation fault 如何处理？与 x86 EPT violation 的异同？**

- 合格信号：guest 管 Stage-1（VA→IPA），hypervisor 管 Stage-2（IPA→PA），硬件两级 walk 合成最终地址；Stage-2 fault trap 到 EL2，读 HPFAR_EL2 得 IPA、ESR_EL2 得访问类型（读/写/取指），据此建立映射或注入 data abort；与 EPT violation 同构（二级翻译缺页 trap 到最高特权层），差异在寄存器命名与 TLB 维护（TLBI vs INVEPT）

- 警示信号：分不清 Stage-1/Stage-2 归属；不知道 fault 信息从哪些寄存器取

**G3.（2 分）GICv3/v4 虚拟化如何工作？List Register 的作用？维护中断何时触发？**

- 合格信号：物理 GIC 经 List Register（ICH_LR_EL2 组）注入虚拟中断给 guest；vGIC 由 hypervisor 软件模拟 distributor/redistributor，硬件经 LR 注入；LR 耗尽或状态变化触发 maintenance interrupt 通知 hypervisor 回收/补充；GICv4 支持 vLPI 直接注入（免 trap）——能对照 x86 APICv/posted interrupt 者佳

- 警示信号：不知道 LR 是虚拟中断注入通道；混淆物理与虚拟中断分发

**G4.（2 分）ARM SMMU（SMMUv3）如何实现 DMA 隔离？设备直通时如何保证 DMA 不越界？**

- 合格信号：SMMU 对设备 DMA 做地址翻译（StreamID→Stage-1/Stage-2），Stage-2 由 hypervisor 配置，把设备 DMA 限制在 guest 物理内存子集；设备直通时 SMMU Stage-2 映射须与 CPU Stage-2 一致（同一 IPA→PA），防止设备 DMA 越界访问其他 guest 或 hypervisor；能对照 VT-d/AMD-Vi 者佳

- 警示信号：认为设备直通不需 SMMU 隔离；不知道 DMA 与 CPU 翻译需一致

**G5.（3 分）跨架构抽象设计题：如何为 x86（VMX/SVM）与 ARM64（EL2）设计统一的 hypervisor 接口？哪些能抽象统一、哪些必须架构特化？**

- 合格信号：可统一——vCPU 生命周期（init/launch/exit 分发）、二级页表接口（map/unmap/protect）、拦截策略语义、hypercall 约定；必须特化——执行流控制原语（VMCS/VMCB vs EL2 系统寄存器）、trap 入口与寄存器现场保存、中断控制器（APIC vs GIC）、TLB 维护指令；分层为公共 trait + 架构后端（如本仓库 intel/amd 双实现，可扩展 arm64）；能指出现有 vendor re-export 模式者佳

- 警示信号：认为可完全统一（忽视硬件差异）；或完全特化（无抽象层意识）

#### 维度 H：软硬协同可信根与隔离计算（权重 8%）

**H1.（3 分）在 CPU 不支持硬件 TEE 的普通服务器上，如何用“软件安全监控器 + 外置 FPGA 可信根”构建可验证的隔离计算？信任根如何建立、度量链如何延伸？**

- 合格信号：FPGA 芯片提供独立于 Host 的设备身份与根密钥（硬件信任根）；可信启动链：芯片校验自身固件→度量安全监控器→监控器接管 Host/页表/设备→度量受保护域软件；信任根从 CPU 厂商解耦到外置芯片，监控器作为 TCB 执行内存/设备隔离（二阶段页表+IOMMU）；形成“硬件身份—环境验证—授权放钥—受保护运行”闭环；能对照 HyperEnclave（TPM RoT + 软件 SGX）者佳

- 警示信号：认为无 CPU-TEE 就无法构建信任根；分不清芯片与监控器的信任职责边界

**H2.（2 分）远程证明中，为什么“Host 自行提交摘要、经芯片签名”不能视为可信环境证据？监控器度量扮演什么角色？**

- 合格信号：Host 不可信（攻击者模型含失陷 Host OS/root），Host 自报度量可被伪造；可信证据必须由 TCB 内的监控器生成（度量自身+受保护域+配置），再经芯片私钥签名（芯片身份不可伪造）；签名只证明“该芯片签署”，内容真实性取决于度量来源是否在可信链内——监控器度量是内容可信的保证

- 警示信号：认为芯片签名即等于环境可信；忽视度量来源的可信性

**H3.（3 分）现场设计题：FPGA 可信根经 PCIe/DMA 受控队列接入，监控器如何安全约束芯片接口、防止不可信 Host 篡改或重放交互？共享页与设备归属如何隔离？**

- 合格信号（按要点累计）：

    1. 芯片接口（PCIe BAR/DMA 队列）由监控器经 IOMMU/SMMU 隔离，Host 不能直接映射或篡改芯片寄存器；

    2. 受控队列内存由监控器管理，Host 请求经监控器校验/编组（防注入），挑战-响应加随机数防重放；

    3. 设备归属：芯片/GPU 经 SMMU Stage-2 绑定到特定运行域，域切换时设备复位+共享页回收清零；

    4. 密钥放行只在验证通过后由芯片封装给获准运行域，工作密钥不进入普通 Host；

    5. 加分：指出 DMA 攻击面（设备可绕过 CPU 直接访存，IOMMU 是必需）、共享页生命周期（建立/回收的清零与映射一致性）

- 警示信号：认为 Host 可信、芯片接口无需隔离；忽视 DMA 越界与重放攻击面

### 三、评分标准

**维度权重与分值**：

|维度|权重|题目|满分|
|---|---|---|---|
|A x86-64 体系结构|12%|A1–A4|7|
|B VMX/SVM 执行模型|20%|B1–B5|11|
|C 拦截机制配置|20%|C1–C5|12|
|D 二级地址翻译（EPT/NPT）|8%|D1–D3|6|
|E Rust 裸机工程|12%|E1–E3|8|
|F 安全思维与设计|8%|F1–F3|7|
|G ARM64 与跨架构虚拟化|12%|G1–G5|12|
|H 软硬协同可信根与隔离计算|8%|H1–H3|8|

**得分换算**：按维度满分归一化为百分制后加权求和。

**等级判定**：

|等级|标准|结论|
|---|---|---|
|Strong Hire|总分 ≥ 85，且 B\+C 合计得分 ≥ 80%，F3 设计题至少 2 分|直接推进，可考虑定级上浮|
|Hire|总分 ≥ 70，且 B\+C 合计得分 ≥ 65%|推进录用|
|Lean Hire|总分 ≥ 60，B\+C 有单题亮点但有知识盲区|视缺口可培养性决定，需二面复核薄弱维度|
|No Hire|总分 \< 60，或 B\+C 合计 \< 50%|终止|

**一票否决项**（任一触发直接 No Hire）：

- B2（VM\-exit 硬件/软件保存分工）完全错误——这属于岗位每日工作的地基

- E3 看不出 transmute 引用的问题且无 unsafe 边界意识

- F1 坚持默认放行且无法被引导到 fail\-closed 的理由上来

**评分纪律**：

- 警示信号 ≠ 直接零分：候选人经提示后能自我修正的，该题按 50% 计分，并在备注记录"可引导性"

- 合格信号超出要点之外的正确延伸，记入加分备注，不突破该题满分

- 每个维度至少作答一题，防止维度跳答掩盖短板

### 四、面试流程建议

|轮次|形式|时长|内容|
|---|---|---|---|
|电话初筛|电话|15 分钟|筛选确认二题 \+ 项目真实性核对|
|一面|技术|90 分钟|维度 A/B/C（B\+C 为重心），B2、C1、C2 必问|
|二面|技术|90 分钟|维度 D/E/F/G（G 按候选人架构侧重选问），E3、F3 必问（现场编码环境备好）|
|三面|架构对话|60 分钟|给出现网约束（x86 双厂商与 ARM64 多平台、5.4/5.10 内核、无 CPU-TEE 硬件、外置 FPGA 可信根），候选人阐述“如何为一个新平台（如鲲鹏 ARM64）从零补齐虚拟化适配与 MSR/系统寄存器策略表”，考察跨架构方法论、规范检索习惯与软硬协同设计能力|

**三面评估要点**：正确路径应为——查架构规范（x86 SDM Vol.4 列 MSR 清单 / ARM ARM 列系统寄存器 trap 清单）→ 按内核实际访问集回归 → 分类定策略 → 生成架构对应的拦截配置（x86 双厂商位图 / ARM HCR_EL2 等 trap 位）→ 在 QEMU 与多架构实机环境验证；软硬协同侧能说明监控器度量如何接入外置 FPGA 可信根的证据链。只凭记忆和经验直接写表的，记入风险备注。

