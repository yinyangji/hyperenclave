# AMD 嵌套 SVM M5 验证计划

## 概述

M5 里程碑目标:在真实硬件环境中验证 L1 能够加载 KVM 并创建 L2 虚拟机,完成嵌套虚拟化的端到端验证。

## 前置条件

### 硬件要求
- AMD/Hygon CPU 支持 SVM (CPUID 0x8000_0001.ECX[2] = 1)
- 启用 NPT (Nested Page Tables)
- 内存 ≥ 8GB (L0 + L1 + L2 需要足够内存)

### 软件环境
- **L0**: HyperEnclave monitor (编译时 `NESTED=on`)
- **L1**: Linux 5.10+ 内核 + KVM-amd 模块
- **L2**: 任意 Linux 发行版 (用于验证启动)

### 编译配置
```bash
# 编译支持嵌套虚拟化的 HyperEnclave
make elf VENDOR=amd SME=on NESTED=on LOG=debug

# 安装到 /lib/firmware
sudo make install VENDOR=amd
```

## 验证阶段

### 阶段 1: L0 启动与嵌套能力暴露 (M1 验证)

**目标**: 确认 L1 能够检测到 SVM 能力。

**步骤**:
1. 加载 hyperenclave-driver:
   ```bash
   sudo modprobe hyperenclave
   ```

2. 在 L1 中检查 CPUID:
   ```bash
   # 检查 SVM 支持 (CPUID 0x8000_0001.ECX[2])
   cpuid -1 -l 0x80000001 | grep -i svm
   
   # 检查嵌套 SVM 特性 (CPUID 0x8000_000A.EDX[3])
   cpuid -1 -l 0x8000000A
   ```

**预期结果**:
- CPUID 0x8000_0001.ECX[2] = 1 (SVM 可用)
- CPUID 0x8000_000A.EDX = 0 (L1 看到 SVM 被禁用,符合 M4 策略)

**调试**:
- 检查 dmesg: `dmesg | grep -i hyperenclave`
- 查看串口日志 (如果配置了): `/var/log/hyperenclave.log`

---

### 阶段 2: L1 加载 KVM-amd (M2 验证)

**目标**: L1 能够成功加载 KVM 并创建 /dev/kvm 设备。

**步骤**:
1. 在 L1 中加载 KVM:
   ```bash
   sudo modprobe kvm-amd nested=0
   ```
   注意: `nested=0` 因为 L2 不能再嵌套 (L0 策略已屏蔽)

2. 检查 KVM 是否加载成功:
   ```bash
   lsmod | grep kvm
   ls -l /dev/kvm
   ```

3. 检查 KVM 能力:
   ```bash
   # 检查 KVM 版本
   cat /sys/module/kvm/version
   
   # 检查 nested 参数
   cat /sys/module/kvm_amd/parameters/nested
   ```

**预期结果**:
- kvm-amd 模块加载成功
- /dev/kvm 设备存在
- nested 参数为 0 (符合预期)

**可能的问题**:
- 如果 KVM 加载失败,检查 dmesg 中的 VMRUN 相关错误
- 如果报 "SVM not available",检查 M1 的 CPUID 暴露是否正确

**调试**:
```bash
# 查看 KVM 初始化日志
dmesg | grep -i kvm

# 查看 L0 的日志 (需要 LOG=debug 编译)
dmesg | grep -i "VMRUN\|VMLOAD\|nested"
```

---

### 阶段 3: L1 创建 L2 虚拟机 (M3 验证)

**目标**: L1 通过 KVM 创建 L2 虚拟机,VMRUN 成功执行。

**步骤**:
1. 准备 L2 镜像 (使用最小化 Linux):
   ```bash
   # 使用 Alpine Linux (轻量级,适合测试)
   wget https://dl-cdn.alpinelinux.org/alpine/v3.18/releases/x86_64/alpine-virt-3.18.0-x86_64.iso
   ```

2. 在 L1 中启动 L2 (使用 QEMU):
   ```bash
   qemu-system-x86_64 \
     -enable-kvm \
     -m 512M \
     -smp 1 \
     -cdrom alpine-virt-3.18.0-x86_64.iso \
     -nographic \
     -append "console=ttyS0"
   ```

3. 观察启动过程:
   - QEMU 创建虚拟机 → KVM ioctl → L1 执行 VMRUN
   - L0 拦截 VMRUN → merge VMCB12 → 设置 VMCB02 → 运行 L2

**预期结果**:
- QEMU 启动成功,不报 KVM 错误
- L2 开始启动 (看到 BIOS/GRUB 或内核启动消息)
- L0 日志中看到 "VMRUN: L1 nCR3=... L2 ASID=..."

**关键断点** (如果 L2 无法启动):
1. **VMRUN 是否被 L0 拦截?**
   - 检查 L0 日志: `dmesg | grep "VMRUN"`
   - 应该看到 "handle_vmrun: ..." 日志

2. **VMCB12 合并是否成功?**
   - 检查 L0 日志: `dmesg | grep "merge_vmcb12_to_vmcb02"`
   - 检查 CR0/CR4/EFER 校验是否通过

3. **nNPT 是否创建成功?**
   - 检查 L0 日志: `dmesg | grep "NestedPageTable"`
   - 应该看到 "get_or_create: L1 nCR3=..."

**调试命令**:
```bash
# 在 L1 中查看 KVM 事件
perf kvm stat live

# 在 L0 中查看 VMCB02 状态 (需要自定义 debugfs 接口)
# cat /sys/kernel/debug/hyperenclave/vmcb02
```

---

### 阶段 4: L2 缺页与 nNPT 合成 (M3 深度验证)

**目标**: L2 执行代码触发 #NPF,L0 正确处理两级翻译。

**步骤**:
1. L2 启动后,尝试访问内存:
   - L2 访问某 GPA → 触发 #NPF (nNPT 缺页)
   - L0 拦截 #NPF → 执行两级 walk:
     * L2 GPA → (L1 nNPT) → L1 GPA
     * L1 GPA → (L0 GPM) → HPA
   - L0 合成 nNPT 页表项: L2 GPA → HPA

2. 观察 L0 日志:
   ```bash
   dmesg | grep "L2 #NPF"
   ```

**预期结果**:
- L2 #NPF 被 L0 成功处理
- 日志显示 "L2 #NPF resolved: L2GPA=... L1 nCR3=..."
- L2 继续执行,不卡住

**可能的问题**:
- **L2 无限循环 #NPF**: L1 nNPT 缺页,需要反射给 L1
  - 检查日志: "L2 #NPF cannot be resolved, reflecting to L1"
  - 这是正常的,L1 的 KVM 会填充 nNPT 后重试

- **L2 访问 sinkhole**: L1 GPA 不在 GPM 中
  - L0 映射到 ff_page (全 0xFF)
  - L2 读到 0xFF,写被丢弃

**调试**:
```bash
# 查看 nNPT 统计 (需要自定义接口)
# cat /sys/kernel/debug/hyperenclave/nnpt_stats
# 预期看到: entries=N, faults=M, reflects=K
```

---

### 阶段 5: L2 执行 CPUID/MSR/IOIO (M4 策略验证)

**目标**: L2 执行敏感指令,L0 正确应用策略并反射。

**步骤**:
1. 在 L2 中执行 CPUID:
   ```bash
   # 在 L2 启动后,运行:
   cpuid -1 -l 0x8000000A
   ```

2. 观察 L0 日志:
   ```bash
   dmesg | grep "CPUID.*masked"
   ```

**预期结果**:
- L2 执行 CPUID 0x8000_000A → L0 拦截
- L0 应用策略: 将 SVM 特性清零
- L0 反射给 L1: 写 VMCB12.exit_code = CPUID
- L1 的 KVM 处理 CPUID,返回清零的结果给 L2
- L2 看到 SVM = 0 (正确)

**测试 MSR 策略**:
```bash
# 在 L2 中尝试读 VM_HSAVE_PA (应该被拒绝)
# rdmsr 0xC0010117  # 如果有 msr-tools
```

**预期结果**:
- L2 尝试访问 VM_HSAVE_PA → L0 拦截
- L0 应用策略: check_msr_policy() 返回 EACCES
- L0 注入 #GP(0) 到 L2
- L2 看到 #GP 异常 (正确)

**测试 PCI 配置空间访问** (如果 L2 有设备):
```bash
# 在 L2 中扫描 PCI 设备
lspci -v
```

**预期结果**:
- L2 访问 0xCF8/0xCFC → L0 拦截
- L0 策略允许反射
- L0 反射给 L1
- L1 的设备模拟返回虚拟 PCI 配置
- L2 看到虚拟设备 (正确)

---

### 阶段 6: L2 退出反射 (M4 完整验证)

**目标**: L2 的各类退出被正确分类并反射给 L1。

**步骤**:
1. 在 L2 中触发不同类型的退出:
   - **CPUID**: `cpuid -1 -l 0x1`
   - **HLT**: (等待 L2 空闲)
   - **异常**: 触发 #PF (访问未映射内存)
   - **中断**: (外部中断到达 L2)

2. 检查 L0 日志:
   ```bash
   dmesg | grep "Reflected L2 exit to L1"
   ```

**预期结果**:
- 每个 L2 退出都被 L0 正确处理:
  - L0 处理: VMMCALL, VMRUN (如果 L2 尝试嵌套)
  - L0 反射: CPUID, HLT, 异常, 中断
- L0 日志显示 "Reflected L2 exit to L1: exit_code=..."
- L1 的 KVM 看到正确的 exit_code 和 exit_info

**VMCB01 恢复验证**:
- L2 退出后,L1 继续运行
- L1 的 RIP/RSP/段寄存器等状态正确恢复
- L1 能够再次执行 VMRUN 启动 L2

---

## 成功标准

### M5 最低通过标准
✅ L1 能够加载 KVM-amd 模块  
✅ L1 能够执行 VMRUN (L0 正确合并 VMCB12)  
✅ L2 能够启动到 BIOS/GRUB  
✅ L2 的 #NPF 被 L0 正确处理 (nNPT 合成)  
✅ L2 退出能够反射给 L1 (VMCB01 恢复)  

### M5 完整通过标准
✅ L2 能够启动到 shell  
✅ L2 能够执行基本命令 (ls, echo, cat)  
✅ L2 的 CPUID/MSR/IOIO 策略正确应用  
✅ L2 无法检测到 SVM (策略屏蔽成功)  
✅ L2 无法访问 L0 私有 MSR (注入 #GP 成功)  
✅ L1 能够多次启动/停止 L2 (状态机稳定)  

---

## 已知限制

根据 `docs/nested-svm-design.md` §10:

1. **单 L2 限制**: 每个 CPU 同时只能运行一个 L2
2. **ASID 简化**: L2 ASID 单调递增,不回收
3. **nNPT 全量失效**: TLB 失效是整棵树,不支持单页失效
4. **L2/Enclave 互斥**: L2 运行时 Enclave 被暂停
5. **无虚拟 IOMMU**: L2 看不到 IOMMU,设备透传由 L0 hypercall 控制

这些限制不影响 MVP 验证,在生产优化阶段处理。

---

## 故障排查

### L2 无法启动

**症状**: QEMU 报 "Could not initialize KVM" 或 L2 卡在启动

**排查步骤**:
1. 检查 L1 的 KVM 是否正常:
   ```bash
   ls -l /dev/kvm
   lsmod | grep kvm
   ```

2. 检查 L0 是否拦截 VMRUN:
   ```bash
   dmesg | grep "VMRUN"
   ```
   应该看到 "handle_vmrun: ..." 日志

3. 检查 VMCB 合并是否失败:
   ```bash
   dmesg | grep "merge_vmcb12_to_vmcb02"
   ```
   如果看到 "CR0/CR4/EFER invalid",说明 L1 的 VMCB12 状态非法

4. 检查 nNPT 创建:
   ```bash
   dmesg | grep "NestedPageTable"
   ```

**常见原因**:
- L1 nCR3 指向无效 GPA → L0 无法创建 nNPT
- L1 设置了 L0 不支持的 CR0/CR4 位 → 合并失败
- L0 的 GPM 没有映射 L1 的某些 GPA → sinkhole,L2 读到 0xFF

---

### L2 启动后立即崩溃

**症状**: L2 进入 BIOS 后 panic 或重启循环

**排查步骤**:
1. 检查 L2 #NPF 处理:
   ```bash
   dmesg | grep "L2 #NPF"
   ```
   如果看到大量 #NPF 但没有 "resolved",可能是两级翻译失败

2. 检查 L1 nNPT 是否有效:
   - L1 的 nCR3 应该指向有效的 nNPT 根
   - L1 的 nNPT 应该映射了 L2 访问的 GPA 范围

3. 启用详细日志重新编译:
   ```bash
   make elf VENDOR=amd SME=on NESTED=on LOG=trace
   ```

**常见原因**:
- L1 nNPT 未正确初始化 (KVM 的 bug 或版本不兼容)
- L2 访问的 GPA 不在 L1 nNPT 中 → 反射给 L1 → L1 未处理 → L2 再次 #NPF → 死循环

---

### L2 能启动但无法执行命令

**症状**: L2 启动到 shell,但执行命令卡住或报错

**排查步骤**:
1. 检查 L2 的 IOIO 访问:
   ```bash
   dmesg | grep "L2.*IOIO\|port"
   ```
   看是否有端口访问被错误拦截

2. 检查 L2 的中断:
   - L2 可能在等待中断 (timer, keyboard)
   - L0 应该反射中断给 L1

3. 在 L2 中检查系统状态:
   ```bash
   # 在 L2 shell 中
   cat /proc/interrupts
   dmesg
   ```

**常见原因**:
- L0 的 IOPM 策略过于严格,拦截了 L2 需要的端口
- 中断反射未实现或有 bug

---

## 下一步

M5 验证通过后,进入 **M6 裸机验证** 和 **Intel VMX 对称实现**:

1. **M6 裸机验证**:
   - 在真实 Hygon 硬件上重复 M5 测试
   - 验证设备隔离 (GPU 对 L1 不可见)
   - 验证 L2 通过 L0 hypercall 获得 GPU 透传

2. **Intel VMX 对称实现**:
   - 将 M1-M4 逻辑移植到 `src/arch/x86_64/intel/`
   - 实现 VMCS12 合并 (对应 VMCB12)
   - 实现 EPT12 合成 (对应 nNPT)
   - 实现 VM exit 反射

3. **优化与文档**:
   - 优化 nNPT 缓存策略
   - 实现 ASID 回收
   - 编写用户手册与 API 文档

---

## 参考

- 设计文档: [`docs/nested-svm-design.md`](nested-svm-design.md)
- M1-M4 实现: `src/arch/x86_64/amd/nested.rs`, `vmcb_merge.rs`, `exit_reflection.rs`
- AMD APM Vol 2: Nested Paging, VMCB layout
- Linux KVM: `arch/x86/kvm/svm/nested.c` (参考实现)
