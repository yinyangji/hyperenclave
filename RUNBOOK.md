# HyperGPU 知乎分享对齐与 QEMU 测试任务手册

> 本文件记录一项进行中的任务，用于在换机器/换环境时恢复上下文。
> 创建时间：2026-10-12
> 负责人：cpy
> 仓库分支：master
> 远程：origin https://github.com/yinyangji/hyperenclave.git
> 最后更新 commit：ac2f7058a3f4f6485916e36b6a0666e964abce1d

---

## 1. 任务目标

1. **永久记录参考资料**：将知乎专栏文章
   *HyperGPU：赋能通用 GPU 设备，构筑大模型隐私计算算力底座*
   （https://zhuanlan.zhihu.com/p/1948802980628394981，作者：徐基明博士，蚂蚁密算科技，出品：DataFun）
   对齐到本仓库设计文档中，并作为后续实现的参考。

2. **补齐 QEMU 级测试验证**：原文描述的是 Hygon 硬件真机测试；本任务要求
   **Intel 路径也必须在 QEMU 嵌套虚拟化环境下跑通验证**。

---

## 2. 已完成

- [x] `ac2f705` 已 push master：修正 `hv_result_err!` 宏误用 + rustfmt。
- [x] 知乎文章已解析；本地保存了完整网页 `docs/hypergpu/hypergpu.mhtml`
      （注意：其中 14 张架构图仍为远程图片 URL，尚未转存到仓库）。
- [x] 已确认 hyperenclave-driver 版本 `v1.4.1`，与 `VERSION` 一致。
- [x] 本机环境勘察：
  - CPU：Intel，支持 VMX，22 vCPU，30 GB 内存
  - `/dev/kvm` 可用
  - QEMU 10.2.1
  - 代理：http://127.0.0.1:7890
- [x] 已启动 Ubuntu 20.04 cloud image 下载：
  `~/tmp/he-qemu/focal-server-cloudimg-amd64.img`

---

## 3. 当前阻断（必须先解决）

`make elf` 双厂商均失败，根源在 `src/memory/nested_pt.rs` 被无条件编译进
公共 `memory` 模块，但代码只适用于 `feature = "nested"` 路径。

典型编译错误：

```text
error[E0425]: cannot find value `EACCES` in this scope
error[E0433]: failed to resolve: could not find `PTF` in `paging`
error[E0599]: no method named `as_usize` found for type `usize`
error[E0599]: no method named `get` found for struct `ROOT_CELL`
error[E0599]: no method named `translate` found for struct `Level4PageTableImmut<VA, PTE>`
error[E0038]: the trait `GenericPageTableMut` is not dyn compatible
```

**推荐修复**：给 `src/memory/nested_pt.rs` 加上 `#[cfg(feature = "nested")]`
门控，并在 `src/memory/mod.rs` 中对 `pub mod nested_pt` 和
`pub use nested_pt::{...}` 同步加 `#[cfg(feature = "nested")]`。

---

## 4. 下一步清单

### 4.1 代码/构建

- [ ] 修复 `nested_pt.rs` 的 feature 门控，恢复双厂商 `make elf`。
- [ ] 跑通质量门禁：
      ```bash
      make format
      make clippy VENDOR=intel SME=off
      make clippy VENDOR=amd SME=on
      make test
      ```

### 4.2 文档记录（知乎分享对齐）

- [ ] 新建 `docs/hypergpu/hypergpu-datafun-talk.md`：
  - 文章标题、讲者、编辑、出品社区、URL
  - 章节结构（背景 / HyperGPU 设计 / 性能 / 展望总结）
  - 与仓库实现的对照表
  - 架构图映射（仓库 `docs/arch-images/*.png`）
- [ ] 更新 `docs/hypergpu-gap-analysis.md`：
  - 修正 G9：不是 PCIe IDE，而是“软件总线加密（SM4）”
  - 修正 G10：不是 GPU 厂商 attestation，而是“设备指纹认证 + SM2/SM3”
  - 补性能与规模基线（L0 约 1 万行、CVM 约 1000 多行、推理性能损耗约 1%、
    总线加密损耗 H20 约 8.2%、指纹采集 0.2s、验证 0.5s、密钥协商 0.2s）
- [ ] 更新 `docs/device-isolation-design.md`：在参考章节加入该知乎链接。
- [ ] 更新 `AGENTS.md` §8 参考资料：加入该知乎链接。

### 4.3 QEMU 测试验证（Intel 路径）

- [ ] 等下载完成后，创建 cloud-init ISO，启动 L1 guest：
  ```bash
  qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 6G \
    -machine q35,kernel-irqchip=split \
    -drive file=~/tmp/he-qemu/focal-server-cloudimg-amd64.img,format=raw,if=virtio \
    -cdrom ~/tmp/he-qemu/cloud-init.iso \
    -device intel-iommu,intremap=on \
    -device ivshmem-plain,memdev=ivshmem,bus=pcie.0 \
    -object memory-backend-ram,size=256M,id=ivshmem \
    -serial stdio
  ```
  （ivshmem 仅作假加速卡占位用，也可替换为 PCI 透传的 GPU/NIC。）
- [ ] 在 L1 内安装 Linux 5.4（或 5.10）内核 headers，编译并加载
      `hyperenclave-driver`，加载 `rust-hypervisor-intel` firmware。
- [ ] 收集 L0/L1 dmesg，验证：
  - `dmesg | grep -i hyperenclave` 正常
  - L1 `lspci` 中受保护设备不可见或 vendor ID 为 0xFFFF
  - 受保护设备的 BAR 读返回全 0xFF
  - 0xCF8/0xCFC 访问被拦截的日志

### 4.4 输出测试指南

- [ ] 新建 `docs/qemu-nested-test-guide.md`：
  - Intel 与 Hygon 两套 QEMU 命令
  - 内核参数（`memmap=...$0x100000000 iommu=off intremap=off no5lvl`）
  - 预期日志、排障步骤、本次实测结果

---

## 5. 关键路径速查

```bash
# 仓库
cd ~/projects/gpu/hyperenclave
git pull

# 驱动
cd ~/projects/soft-tee/hyperenclave-driver

# 下载目录
ls -la ~/tmp/he-qemu/

# 本机代理（若网络不通需重设）
http_proxy=http://127.0.0.1:7890
https_proxy=http://127.0.0.1:7890

# 门禁命令
make format
make clippy VENDOR=intel SME=off
make clippy VENDOR=amd SME=on
make test
make elf VENDOR=intel SME=off LOG=info
make elf VENDOR=amd SME=on LOG=info
```

---

## 6. 相关文件

- 知乎文章本地副本：`docs/hypergpu/hypergpu.mhtml`
- 已有架构图：`docs/arch-images/hypergpu*.png`、`io.png`、`isolate.png`、`pcie-encrypt.png`
- 嵌套 AMD 验证计划：`docs/nested-svm-m5-verification.md`
- 设备隔离设计：`docs/device-isolation-design.md`
- 差距分析：`docs/hypergpu-gap-analysis.md`
- 仓库开发规范：`AGENTS.md`、`CLAUDE.md`

---

## 7. 注意事项

- `src/memory/nested_pt.rs` 为安全关键代码，改动后需走
  `tcb-security-reviewer` 审查。
- 修改 `src/arch/x86_64/` 后需检查 Intel/AMD 对称性，必要时运行
  `vendor-parity-reviewer`。
- 不要直接 push，提交前跑 `make format-check` 与 `make clippy/test/elf`。
