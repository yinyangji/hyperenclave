# 虚拟化核心研发工程师（Hypervisor · Rust）

> **文档使用说明**：本文档分两部分。第一部分为对外 JD，可直接发布至招聘渠道；第二部分为面试题库与评分标准，仅供内部面试官使用，不对外发布。

---

## 第一部分 对外 JD

### 岗位名称

虚拟化核心研发工程师（跨架构 Hypervisor · GPU 隔离 · Rust）

### 岗位简介

本岗位服务于基于 Rust 的裸机 hypervisor 安全监控器研发。监控器运行于 CPU 最高特权层（x86-64 Intel VT-x / AMD SVM root 模式，ARM64 EL2），在缺少硬件 TEE 的通用服务器上，以软件方式在宿主系统与物理硬件之间建立强制隔离边界。岗位核心目标是把高价值加速设备（尤其是 GPU 显卡）从宿主系统的可达范围中结构性剥离：宿主操作系统及其管理员不再能访问 GPU，GPU 仅按策略授权给特定受保护虚拟机独占直通使用。隔离依托二级地址翻译（EPT/NPT/Stage-2）与 IOMMU/DMA 重映射（VT-d/AMD-Vi/SMMU）实现——GPU 的 MMIO/BAR 与 DMA 通路只对授权 VM 可见，对宿主与其他 VM 结构性不可达。监控器并与外置硬件可信根（FPGA 类安全芯片）软硬协同，完成度量启动、远程证明与密钥放行。虚拟化层需覆盖鲲鹏、飞腾（ARM64）与海光、AMD、Intel（x86-64）多平台。

### 工作内容

- 设计与实现 GPU 设备隔离与专属直通（核心方向）：把 GPU 从宿主二级地址翻译视图与 DMA 通路中剥离，按策略将 GPU 的 MMIO/BAR、DMA 与 MSI-X 中断独占绑定到授权 VM，实现宿主 GPU 访问剥夺、设备归属仲裁、VM 生命周期内的 GPU 独占与释放

- 实现设备复位与状态清理：VM 切换或释放时执行 GPU 功能级复位（FLR）、显存与 BAR 共享页回收清零、IOMMU 映射与中断重映射解除，防止跨 VM 与宿主的信息残留与越界访问

- 实现 IOMMU 与 DMA 隔离：VT-d/AMD-Vi/SMMU 重映射配置，强制设备 DMA 限定在授权域内存，抵御恶意设备 DMA 越界

- 实现二级地址翻译视图管理：EPT/NPT/Stage-2 页表、安全内存空映射、设备 MMIO 映射、保护域双视图切换、大页映射策略

- 维护与扩展多架构执行流控制：x86 VMCS/VMCB 拦截位图、异常注入、CR0/CR4 guest-host mask 与 read shadow；ARM64 EL2 配置（HCR_EL2 trap 位、Stage-2 翻译表、VHE 宿主内核运行）

- 设计与实现 MSR/系统寄存器虚拟化子系统：拦截策略表、x86 MSR bitmap 与 VMCB MSRPM 双厂商位图生成、VMCS MSR load/store area 硬件切换、ARM64 系统寄存器 trap 配置，按寄存器逐项定义仿真语义

- 建设 CPUID/ID 寄存器策略引擎：启动期快照、特性位掩码、拓扑枚举受控视图

- 对接外置硬件可信根：经 PCIe/DMA 受控队列与 FPGA 安全芯片交互，协同完成度量启动、远程证明与密钥放行

- 建设跨架构抽象层与多平台适配矩阵：在 x86（VMX/SVM）与 ARM64（EL2）间提炼统一虚拟化接口，针对鲲鹏/飞腾/海光/AMD/Intel 完成适配与验证

- 在 QEMU 嵌套虚拟化环境与多架构实机环境搭建并维护测试矩阵

### 任职要求

1. **计算机体系结构**：深入理解 x86-64 长模式分页（四级页表、TLB 语义与刷除时机）、段机制与特权级、LAPIC/x2APIC 中断体系、IDT 与异常错误码模型、EFER/PAT/MTRR/STAR/SYSENTER 族 MSR 语义；或深入理解 ARM64 异常级别（EL0–EL3）、Stage-1/Stage-2 地址翻译、VHE、GICv3/v4 中断体系、系统寄存器编码与 trap 机制。两者至少精通其一，并具备向另一架构迁移的能力

2. **硬件虚拟化执行模型**：精通 Intel VMX（VMCS guest/host/control 三区字段、vmlaunch/vmresume 状态机、instruction error 排障）、AMD SVM（VMCB save/control 区、vmrun/vmload/vmsave）或 ARM EL2 虚拟化（HCR_EL2 trap 控制、Stage-2 fault 处理、EL2 异常向量）至少一种，理解特权层切换时硬件自动保存与恢复的状态集合

3. **拦截机制配置**：具备 MSR bitmap、IO bitmap、异常位图、CR guest-host mask 与 read shadow、VM-entry/exit MSR load/store area（x86），或 HCR_EL2 trap 位、CPTR_EL2、Stage-2 权限控制（ARM）中至少三项的实际配置经验

4. **二级地址翻译**：掌握 EPT/NPT（x86）或 Stage-2（ARM）页表层级与大页映射、violation/fault 限定字段解析，理解 INVEPT/INVLPGA（x86）或 TLBI 指令（ARM）的适用时机

5. **设备与 DMA 虚拟化**：理解 PCIe 设备模型（BAR/MMIO、配置空间、MSI/MSI-X 中断）、设备直通与归属管理，掌握 VT-d/AMD-Vi/SMMU 的 DMA 重映射与中断重映射机制，能够分析设备 DMA 越界攻击面并给出隔离方案

6. **Rust 裸机工程**：熟练进行 no_std 环境开发，掌握内联汇编与 naked function 编写，能够为 unsafe 抽象给出健全性论证

7. **规范精读能力**：能够按章节定位并精确解读 Intel SDM Vol.3C、AMD APM Vol.2 或 ARM Architecture Reference Manual（DDI 0487）的虚拟化相关章节，工程决策以架构规范为准绳

### 优先条件

- 具备 GPU/加速卡虚拟化经验：NVIDIA/AMD GPU 直通、vGPU、SR-IOV、GPU MMIO/BAR 重映射或显存隔离任一

- 具备 VT-d、AMD-Vi 或 ARM SMMU（SMMUv3）DMA 重映射与中断重映射配置实践

- 熟悉 vfio 设备直通框架、PCIe ACS、IOMMU group 与设备功能级复位（FLR）机制

- 具备国产 CPU 平台（鲲鹏、飞腾、海光）虚拟化适配或 bring-up 经验

- 熟悉 GICv3/v4 虚拟化（vGIC、List Register、ITS）、APICv、posted interrupt、AVIC 任一中断虚拟化机制

- 研读过 KVM（arch/x86 或 arch/arm64）、hvpp、SimpleVisor、Jailhouse 等开源 hypervisor 实现源码

- 具备外置硬件可信根协同经验：FPGA/TPM 类安全芯片对接、度量启动、远程证明、密钥管理

- 参与过 TEE、SGX、TDX、SEV-SNP、ARM CCA、GPU TEE 或机密计算方向项目

- 有 QEMU 嵌套虚拟化调试经验，掌握 GDB remote 与串口日志排障方法

- 对 Coq 等形式化验证工具有基础

- 有 HyperEnclave、HyperGPU 等开源机密计算项目研究经验

---

## 第二部分 内部面试材料（不对外发布）

### 一、简历筛选要点

**强信号（直接进入面试）**：

- 向 KVM/QEMU/Xen 等社区提交过虚拟化相关 patch（附 commit 链接）

- 独立编写或深度参与过 hypervisor 项目（课程实现、研究原型、开源项目均可，需能说明细节）

- 工作经历中含 VMM、IOMMU、GPU/设备直通、机密计算（TDX/SEV\-SNP）任一方向

- 技术博客或论文中引用 SDM/APM/ARM ARM 具体章节讨论过虚拟化细节

- 具备 ARM64 EL2 虚拟化（Stage-2、GICv3/v4、SMMU）或国产 CPU 平台（鲲鹏/飞腾/海光）适配经验

- 有外置硬件可信根（FPGA/TPM）软硬协同、度量启动或远程证明实现经验

- 有 GPU 设备隔离与直通研发经历：GPU 直通/vGPU、SR-IOV、vfio、显存与 BAR 隔离、设备 DMA 隔离任一

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

#### 维度 A：x86\-64 体系结构（权重 10%）

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

#### 维度 B：VMX/SVM 执行模型（权重 18%）

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

#### 维度 C：拦截机制配置（权重 16%）

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

#### 维度 D：二级地址翻译（EPT/NPT）（权重 10%）

**D1\.（2 分）EPT violation 的 exit qualification 里有哪些关键字段？软件如何区分读/写/执行违规？**

- 合格信号：违规的 guest 物理地址、访问类型位（读/写/取指）、GVA 可选提供（enable EPT violations \#VE 或 GVA translation）、final translation 位；据此决定注入 guest \#PF、建立映射或走保护域逻辑

- 警示信号：不了解 exit qualification 携带访问类型

**D2\.（2 分）INVEPT 的两种类型（single\-context / all\-context）各适用什么场景？**

- 合格信号：修改了某 EPTP 的页表后可单上下文失效（按 EPTP），全局性变更（如重映射安全内存）用 all\-context；能对照 INVLPGA（SVM 按 ASID\+地址）者佳

- 警示信号：不知道为什么不能用普通 INVLPG（那是 guest 线性地址语义）

**D3\.（2 分）guest 写 CR3 应该拦截吗？KVM 与本项目为何选择不同？**

- 合格信号：KVM 拦截 CR3（影子页表时代遗留 \+ 分时复用需要跟踪切换 vCPU 上下文）；本项目 1:1 直通（每 vCPU 对应固定 pCPU，guest 页表自管，CR3 写无需仲裁，EPT 兜底物理边界）——考察对"拦截面 = 越权面"原则的理解深度

- 警示信号：认为必须拦截 CR3（未理解分区直通模型与二级翻译的兜底关系）

#### 维度 E：Rust 裸机工程（权重 10%）

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

#### 维度 F：安全思维与综合设计（权重 6%）

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

#### 维度 G：ARM64 EL2 与跨架构虚拟化（权重 10%）

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

#### 维度 I：GPU 设备隔离与专属直通（权重 12%）

**I1.（2 分）如何把 GPU 从宿主系统强制隔离，使宿主操作系统与管理员都无法访问 GPU？涉及哪些机制？**

- 合格信号：宿主二级页表（EPT/NPT/Stage-2）不映射 GPU 的 MMIO/BAR（结构性挖洞），宿主对 GPU 配置空间与寄存器的访问不可达；IOMMU（VT-d/AMD-Vi/SMMU）不为宿主域配置 GPU 的 DMA 通路；GPU 中断（MSI-X）不经中断重映射路由到宿主；隔离靠“映射不存在”而非逐次拦截，宿主即便 ring0 也无法触达

- 警示信号：认为靠拦截宿主驱动调用即可（未理解结构性剥离）；不知道 MMIO/BAR 与 DMA 两条通路都要断

**I2.（3 分）GPU 仅授权特定 VM 独占直通，需要哪些机制协同？设备归属如何绑定到该 VM？**

- 合格信号（按要点累计）：

    1. GPU MMIO/BAR 只映射进授权 VM 的二级页表（GPA→GPU 物理 BAR），其他 VM 与宿主的二级页表均不含；

    2. IOMMU/SMMU Stage-2 把 GPU 的 DMA 限定到授权 VM 的内存（GPA→HPA 仅覆盖该 VM），防止 GPU DMA 越界；

    3. GPU 的 MSI-X 中断经中断重映射（VT-d IR / SMMU ITS）路由到该 VM 的 vCPU；

    4. 设备归属由监控器仲裁并登记（一个 GPU 同一时刻只绑定一个域），绑定/解绑经 hypercall；

    5. 加分：指出 GPU 配置空间访问、PCIe ACS（防 peer-to-peer DMA 绕过 IOMMU）、显存 aperture 的隔离

- 警示信号：只答“把 GPU 给 VM”说不出 MMIO/DMA/中断三条通路的绑定；忽视设备归属的唯一性仲裁

**I3.（3 分）VM 释放或切换 GPU 时，如何防止信息残留与越界？前一个 VM 的显存数据会不会泄露给下一个 VM 或宿主？**

- 合格信号（按要点累计）：

    1. GPU 功能级复位（FLR）清空设备内部状态与引擎上下文；

    2. 显存与 BAR 映射的共享页在解绑时回收并清零（防显存残留前一 VM 数据）；

    3. 解除该 VM 的 IOMMU/SMMU Stage-2 映射与中断重映射条目，使 GPU 对原 VM 不再可达；

    4. 失效相关 TLB（IOTLB \+ CPU 二级翻译 TLB），确保旧映射不被缓存复用；

    5. 加分：指出复位与清零的时序（先解除映射再复位，防复位窗口 DMA）、GPU 固件/引擎残留状态、多 GPU/NVLink 拓扑下的归属一致性

- 警示信号：认为解绑只需删映射（漏掉显存清零与设备复位）；不知道 IOTLB 失效的必要性

**I4.（2 分）为什么 IOMMU 是 GPU 隔离的必需而非可选？恶意设备 DMA 的攻击面是什么？**

- 合格信号：设备可绕过 CPU 直接发起 DMA 访存，若仅靠 CPU 二级页表隔离而 GPU/设备 DMA 不受 IOMMU 约束，被隔离的 GPU 或恶意设备仍可 DMA 读写宿主或其他 VM 内存，摧毁隔离；IOMMU 对设备 DMA 做与 CPU 二级翻译同构的地址重映射，把 DMA 限定在授权域；对照 HyperGPU 威胁模型——可抵抗管理员提权、隔离 VM 串通、恶意设备 DMA，前提正是 IOMMU 强制 \+ 二级页表结构性剥离

- 警示信号：认为 CPU 二级页表隔离已足够（忽视 DMA 是独立攻击面）；不知道设备可绕过 CPU 访存

### 三、评分标准

**维度权重与分值**：

|维度|权重|题目|满分|
|---|---|---|---|
|A x86-64 体系结构|10%|A1–A4|7|
|B VMX/SVM 执行模型|18%|B1–B5|11|
|C 拦截机制配置|16%|C1–C5|12|
|D 二级地址翻译（EPT/NPT）|10%|D1–D3|6|
|E Rust 裸机工程|10%|E1–E3|8|
|F 安全思维与设计|6%|F1–F3|7|
|G ARM64 与跨架构虚拟化|10%|G1–G5|12|
|H 软硬协同可信根与隔离计算|8%|H1–H3|8|
|I GPU 设备隔离与专属直通|12%|I1–I4|10|

**得分换算**：按维度满分归一化为百分制后加权求和。

**等级判定**：

|等级|标准|结论|
|---|---|---|
|Strong Hire|总分 ≥ 85，且 B\+C 合计得分 ≥ 80%、I 维度 ≥ 70%，F3/I2/I3 任一设计题至少 2 分|直接推进，可考虑定级上浮|
|Hire|总分 ≥ 70，且 B\+C 合计得分 ≥ 65%、I 维度不低于 50%|推进录用|
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
|二面|技术|90 分钟|维度 D/E/F/G/I（G 按候选人架构侧重选问，I 为 GPU 隔离核心必问），E3、F3、I2 必问（现场编码环境备好）|
|三面|架构对话|60 分钟|给出现网约束（无 CPU/GPU 硬件 TEE、x86 双厂商与 ARM64 多平台、外置 FPGA 可信根），候选人阐述“如何把 GPU 从宿主系统强制隔离、仅授权特定 VM 独占直通”的完整设计——覆盖二级页表剥离、IOMMU/SMMU DMA 绑定、MSI-X 中断重映射、VM 切换时 GPU 复位与显存/BAR 回收清零、恶意设备 DMA 防护，考察设备隔离方法论、威胁建模与软硬协同设计能力|

**三面评估要点**：GPU 隔离设计的正确路径应为——宿主二级页表与 IOMMU 视图均不含 GPU（结构性剥离，而非拦截宿主驱动调用）→ GPU MMIO/BAR 与 DMA 仅映射进授权 VM 的二级页表/SMMU Stage-2 → MSI-X 中断经中断重映射路由到该 VM → 设备归属唯一仲裁、绑定/解绑经 hypercall → VM 释放时先解除映射再 GPU FLR 复位、显存与共享页清零、IOTLB 失效 → 恶意设备 DMA 由 IOMMU 强制拦截（对照 HyperGPU 威胁模型：可抵抗管理员提权、隔离 VM 串通、恶意设备 DMA）。跨架构与策略表侧：查架构规范（x86 SDM Vol.4 列 MSR 清单 / ARM ARM 列系统寄存器 trap 清单）→ 按内核实际访问集回归 → 分类定策略 → 生成架构对应的拦截配置 → 在 QEMU 与多架构实机环境验证；软硬协同侧能说明监控器度量如何接入外置 FPGA 可信根的证据链。只凭记忆和经验直接写表、或认为 CPU 二级页表隔离即可无视 DMA 攻击面的，记入风险备注。

