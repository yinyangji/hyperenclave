// Copyright (C) 2023 Ant Group CO., Ltd. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::fmt::{Debug, Formatter, Result};

use libvmm::msr::Msr;
use libvmm::svm::flags::{InterruptType, VmcbCleanBits, VmcbIntInfo, VmcbTlbControl};
use libvmm::svm::{vmcb::VmcbSegment, SvmExitCode, SvmIntercept, Vmcb};
use x86::{segmentation, segmentation::SegmentSelector, task};
use x86_64::addr::VirtAddr;
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
use x86_64::registers::model_specific::{Efer, EferFlags};
use x86_64::registers::rflags::RFlags;
use x86_64::structures::DescriptorTablePointer;

use crate::arch::msr::policy::{self, MsrAction};
use crate::arch::segmentation::Segment;
use crate::arch::vmm::VcpuAccessGuestState;
use crate::arch::{GuestPageTableImmut, GuestRegisters, LinuxContext};
use crate::cell::Cell;
use crate::error::HvResult;
use crate::memory::addr::{phys_encrypted, virt_to_phys};
use crate::memory::{Frame, GenericPageTableImmut};
use crate::percpu::PerCpu;

#[repr(C)]
pub struct Vcpu {
    /// Save guest general registers when handle VM exits.
    guest_regs: GuestRegisters,
    /// RSP will be loaded from here when handle VM exits.
    host_stack_top: u64,
    /// host state-save area.
    host_save_area: Frame,
    /// MSR permission map backing the MSR_PROT interception.
    msrpm: MsrPermissionMap,
    /// Virtual machine control block.
    pub(super) vmcb: Vmcb,
}

/// AMD SVM MSR permission map (APM Vol 2, §15.10): three 2048-byte ranges
/// covering 0x0000_0000..=0x0000_1FFF, 0xC000_0000..=0xC000_1FFF and
/// 0xC001_0000..=0xC001_1FFF (6144 bytes total). Two permission bits per
/// MSR (LSB = read, bit 1 = write); a set bit means "intercept".
///
/// Like the Intel MSR bitmap, the map starts zeroed (bare-metal semantics:
/// everything passes through) and only the policy table's non-passthrough
/// entries set bits, so both vendors encode exactly the same policy.
struct MsrPermissionMap {
    /// Two pages (8 KiB): the smallest allocation that keeps the base
    /// 4-KiB aligned, as VMRUN requires. Only the first 6144 bytes are used.
    frame: Frame,
}

/// Offset (byte, bit) of `msr`'s permission bit inside the 6144-byte map.
/// `None` for MSRs outside the three ranges: they cannot be trapped and
/// keep bare-metal semantics.
fn permission_bit(msr: u32, is_write: bool) -> Option<(usize, u8)> {
    let (range_bytes, msr_base) = match msr {
        0x0..=0x1fff => (0usize, 0u32),
        0xc000_0000..=0xc000_1fff => (2048, 0xc000_0000),
        0xc001_0000..=0xc001_1fff => (4096, 0xc001_0000),
        _ => return None,
    };
    let bit = range_bytes * 8 + (msr - msr_base) as usize * 2 + is_write as usize;
    Some((bit / 8, (bit % 8) as u8))
}

/// Encode the shared MSR policy table into a zeroed permission map.
fn apply_policy(map: &mut [u8]) {
    debug_assert!(map.len() >= 3 * 2048);
    for entry in policy::MSR_POLICY_TABLE.iter() {
        for msr in entry.first..=entry.last {
            if entry.read != MsrAction::Passthrough {
                if let Some((byte, bit)) = permission_bit(msr, false) {
                    map[byte] |= 1 << bit;
                }
            }
            if entry.write != MsrAction::Passthrough {
                if let Some((byte, bit)) = permission_bit(msr, true) {
                    map[byte] |= 1 << bit;
                }
            }
        }
    }
}

impl MsrPermissionMap {
    fn new() -> HvResult<Self> {
        let mut frame = Frame::new_contiguous(2, 12)?;
        frame.zero();
        apply_policy(frame.as_slice_mut());
        Ok(Self { frame })
    }
}

impl Vcpu {
    pub fn new(linux: &LinuxContext, cell: &Cell) -> HvResult<Self> {
        super::check_hypervisor_feature()?;

        // make sure all perf counters are off
        unsafe {
            /// Core Performance Event-Select Register (PerfEvtSeln), Counter Enable (bit 22)
            const PERF_EVT_SEL_EN: u64 = 1 << 22;
            Msr::PERF_EVT_SEL0.write(Msr::PERF_EVT_SEL0.read() & !PERF_EVT_SEL_EN);
            Msr::PERF_EVT_SEL1.write(Msr::PERF_EVT_SEL1.read() & !PERF_EVT_SEL_EN);
            Msr::PERF_EVT_SEL2.write(Msr::PERF_EVT_SEL2.read() & !PERF_EVT_SEL_EN);
            Msr::PERF_EVT_SEL3.write(Msr::PERF_EVT_SEL3.read() & !PERF_EVT_SEL_EN);
            Msr::PERF_EVT_SEL4.write(Msr::PERF_EVT_SEL4.read() & !PERF_EVT_SEL_EN);
            Msr::PERF_EVT_SEL5.write(Msr::PERF_EVT_SEL5.read() & !PERF_EVT_SEL_EN);
        }

        // Check control registers.
        //
        // Reserved bits must be zero (G11) and the monitor only supports
        // paging in long mode: fail fast instead of running into undefined
        // behavior later.
        let cr0 = linux.cr0;
        let cr4 = linux.cr4;
        if cr0.bits() & super::super::CR0_RESERVED != 0
            || cr4.bits() & super::super::CR4_RESERVED != 0
        {
            return hv_result_err!(
                EINVAL,
                format!(
                    "Reserved bits set in CR0/CR4: cr0={:#x}, cr4={:#x}",
                    cr0.bits(),
                    cr4.bits()
                )
            );
        }
        if !cr0.contains(Cr0Flags::PAGING | Cr0Flags::PROTECTED_MODE_ENABLE)
            || !cr4.contains(Cr4Flags::PHYSICAL_ADDRESS_EXTENSION)
        {
            return hv_result_err!(EINVAL, "Hypervisor requires paging in long mode");
        }
        if cr4.contains(Cr4Flags::L5_PAGING) {
            return hv_result_err!(ENODEV, "L5_PAGING isn't supported by hypervisor!");
        }

        let efer = Efer::read();
        if efer.contains(EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE) {
            return hv_result_err!(EBUSY, "SVM is already turned on!");
        }
        let host_save_area = Frame::new()?;
        let msrpm = MsrPermissionMap::new()?;
        unsafe { Efer::write(efer | EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE) };
        unsafe { Msr::VM_HSAVE_PA.write(host_save_area.start_paddr() as _) };
        info!("successed to turn on SVM.");

        // bring CR0 and CR4 into well-defined states.
        unsafe {
            Cr0::write(Cr0::read());
            Cr4::write(Cr4::read() | super::super::HOST_CR4);
        }

        let mut ret = Self {
            guest_regs: Default::default(),
            host_save_area,
            msrpm,
            host_stack_top: PerCpu::from_local_base().stack_top() as _,
            vmcb: Default::default(),
        };
        assert_eq!(
            unsafe { (&ret.guest_regs as *const GuestRegisters).add(1) as u64 },
            &ret.host_stack_top as *const _ as u64
        );
        ret.vmcb_setup(linux, cell);

        Ok(ret)
    }

    pub fn exit(&self, linux: &mut LinuxContext) -> HvResult {
        self.load_vmcb_guest(linux);
        unsafe {
            core::arch::asm!("stgi");
            Efer::write(Efer::read() - EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE);
            Msr::VM_HSAVE_PA.write(0);
        }
        info!("successed to turn off SVM.");
        Ok(())
    }

    pub fn activate_vmm(&mut self, linux: &LinuxContext) -> HvResult {
        let common_cpu_data = PerCpu::from_id(PerCpu::from_local_base().cpu_id);
        let vmcb_paddr = phys_encrypted(virt_to_phys(
            &common_cpu_data.vcpu.vmcb as *const _ as usize,
        ));
        let regs = self.regs_mut();
        regs.rax = vmcb_paddr as _;
        regs.rbx = linux.rbx;
        regs.rbp = linux.rbp;
        regs.r12 = linux.r12;
        regs.r13 = linux.r13;
        regs.r14 = linux.r14;
        regs.r15 = linux.r15;
        unsafe {
            core::arch::asm!(
                "clgi",
                "mov rsp, {0}",
                restore_regs_from_stack!(),
                "vmload rax",
                "jmp {1}",
                in(reg) regs as * const _ as usize,
                sym svm_run,
                options(noreturn),
            );
        }
    }

    pub fn deactivate_vmm(&self, linux: &LinuxContext) -> HvResult {
        self.guest_regs.return_to_linux(linux)
    }

    pub fn inject_fault(&mut self) -> HvResult {
        self.vmcb.inject_event(
            VmcbIntInfo::from(
                InterruptType::Exception,
                crate::arch::ExceptionType::GeneralProtectionFault,
            ),
            0,
        );
        Ok(())
    }

    pub fn advance_rip(&mut self, instr_len: u8) -> HvResult {
        self.vmcb.save.rip += instr_len as u64;
        Ok(())
    }

    pub fn rollback_rip(&mut self, instr_len: u8) -> HvResult {
        self.vmcb.save.rip -= instr_len as u64;
        Ok(())
    }

    pub fn guest_is_privileged(&self) -> bool {
        self.vmcb.save.cpl == 0
    }

    #[allow(dead_code)]
    pub fn in_hypercall(&self) -> bool {
        use core::convert::TryInto;
        matches!(
            self.vmcb.control.exit_code.try_into(),
            Ok(SvmExitCode::VMMCALL)
        )
    }

    pub fn guest_page_table(&self) -> GuestPageTableImmut {
        use crate::memory::addr::align_down;
        unsafe { GuestPageTableImmut::from_root(align_down(self.vmcb.save.cr3 as _)) }
    }
}

impl Vcpu {
    fn set_vmcb_dtr(vmcb_seg: &mut VmcbSegment, dtr: &DescriptorTablePointer) {
        vmcb_seg.limit = dtr.limit as u32 & 0xffff;
        vmcb_seg.base = dtr.base.as_u64();
    }

    fn set_vmcb_segment(vmcb_seg: &mut VmcbSegment, seg: &Segment) {
        vmcb_seg.selector = seg.selector.bits();
        vmcb_seg.attr = seg.access_rights.as_svm_segment_attributes();
        vmcb_seg.limit = seg.limit;
        vmcb_seg.base = seg.base;
    }

    fn vmcb_setup(&mut self, linux: &LinuxContext, cell: &Cell) {
        self.set_cr(0, linux.cr0.bits());
        self.set_cr(4, linux.cr4.bits());
        self.set_cr(3, linux.cr3);

        let vmcb = &mut self.vmcb.save;
        Self::set_vmcb_segment(&mut vmcb.cs, &linux.cs);
        Self::set_vmcb_segment(&mut vmcb.ds, &linux.ds);
        Self::set_vmcb_segment(&mut vmcb.es, &linux.es);
        Self::set_vmcb_segment(&mut vmcb.fs, &linux.fs);
        Self::set_vmcb_segment(&mut vmcb.gs, &linux.gs);
        Self::set_vmcb_segment(&mut vmcb.tr, &linux.tss);
        Self::set_vmcb_segment(&mut vmcb.ss, &Segment::invalid());
        Self::set_vmcb_segment(&mut vmcb.ldtr, &Segment::invalid());
        Self::set_vmcb_dtr(&mut vmcb.idtr, &linux.idt);
        Self::set_vmcb_dtr(&mut vmcb.gdtr, &linux.gdt);
        vmcb.cpl = 0; // Linux runs in ring 0 before migration
        vmcb.rflags = 0x2;
        vmcb.rip = linux.rip;
        vmcb.rsp = linux.rsp;
        vmcb.rax = 0;
        vmcb.sysenter_cs = Msr::IA32_SYSENTER_CS.read();
        vmcb.sysenter_eip = Msr::IA32_SYSENTER_EIP.read();
        vmcb.sysenter_esp = Msr::IA32_SYSENTER_ESP.read();
        vmcb.star = Msr::IA32_STAR.read();
        vmcb.lstar = Msr::IA32_LSTAR.read();
        vmcb.cstar = Msr::IA32_CSTAR.read();
        vmcb.sfmask = Msr::IA32_FMASK.read();
        vmcb.kernel_gs_base = Msr::IA32_KERNEL_GSBASE.read();
        vmcb.efer = linux.efer | EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE.bits(); // Make the hypervisor visible
        vmcb.g_pat = linux.pat;
        vmcb.dr7 = 0x400;
        vmcb.dr6 = 0xffff_0ff0;

        let vmcb = &mut self.vmcb.control;
        vmcb.intercept_exceptions = 0;
        vmcb.np_enable = 1;
        vmcb.guest_asid = 1; // No more than one guest owns the CPU
        vmcb.clean_bits = VmcbCleanBits::empty(); // Explicitly mark all of the state as new
        vmcb.nest_cr3 = cell.gpm.page_table().root_paddr() as _;
        vmcb.tlb_control = VmcbTlbControl::FlushAsid as _;
        // The base must be 4-KiB aligned (a VMRUN validity requirement
        // whenever the MSR_PROT interception is active).
        vmcb.msrpm_base_pa = self.msrpm.frame.start_paddr() as _;

        self.vmcb.set_intercept(SvmIntercept::NMI, true);
        self.vmcb.set_intercept(SvmIntercept::CPUID, true);
        self.vmcb.set_intercept(SvmIntercept::SHUTDOWN, true);
        self.vmcb.set_intercept(SvmIntercept::VMRUN, true);
        self.vmcb.set_intercept(SvmIntercept::VMMCALL, true);
        self.vmcb.set_intercept(SvmIntercept::VMLOAD, true);
        self.vmcb.set_intercept(SvmIntercept::VMSAVE, true);
        self.vmcb.set_intercept(SvmIntercept::STGI, true);
        self.vmcb.set_intercept(SvmIntercept::CLGI, true);
        self.vmcb.set_intercept(SvmIntercept::SKINIT, true);
        self.vmcb.set_intercept(SvmIntercept::MSR_PROT, true);
    }

    fn load_vmcb_guest(&self, linux: &mut LinuxContext) {
        let vmcb = &self.vmcb.save;
        linux.rip = vmcb.rip;
        linux.rsp = vmcb.rsp;
        linux.cr0 = Cr0Flags::from_bits_truncate(vmcb.cr0);
        linux.cr3 = vmcb.cr3;
        linux.cr4 = Cr4Flags::from_bits_truncate(vmcb.cr4);
        linux.efer = vmcb.efer & !EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE.bits();

        linux.cs.selector = SegmentSelector::from_raw(vmcb.cs.selector);
        linux.ds.selector = SegmentSelector::from_raw(vmcb.ds.selector);
        linux.es.selector = SegmentSelector::from_raw(vmcb.es.selector);

        linux.gdt.base = VirtAddr::new(vmcb.gdtr.base);
        linux.gdt.limit = vmcb.gdtr.limit as _;
        linux.idt.base = VirtAddr::new(vmcb.idtr.base);
        linux.idt.limit = vmcb.idtr.limit as _;

        // We should load the following register state manually since we not use VMLOAD/VMSAVE
        linux.fs.selector = segmentation::fs();
        linux.gs.selector = segmentation::gs();
        linux.tss.selector = unsafe { task::tr() };
        linux.fs.base = Msr::IA32_FS_BASE.read();
        linux.gs.base = Msr::IA32_GS_BASE.read();
    }
}

impl VcpuAccessGuestState for Vcpu {
    fn regs(&self) -> &GuestRegisters {
        &self.guest_regs
    }

    fn regs_mut(&mut self) -> &mut GuestRegisters {
        &mut self.guest_regs
    }

    fn instr_pointer(&self) -> u64 {
        self.vmcb.save.rip
    }

    fn stack_pointer(&self) -> u64 {
        self.vmcb.save.rsp
    }

    fn set_stack_pointer(&mut self, sp: u64) {
        self.vmcb.save.rsp = sp
    }

    fn rflags(&self) -> u64 {
        self.vmcb.save.rflags
    }

    fn fs_base(&self) -> u64 {
        self.vmcb.save.fs.base
    }

    fn gs_base(&self) -> u64 {
        self.vmcb.save.gs.base
    }

    fn efer(&self) -> u64 {
        self.vmcb.save.efer
    }
    fn cr(&self, cr_idx: usize) -> u64 {
        match cr_idx {
            0 => self.vmcb.save.cr0,
            3 => self.vmcb.save.cr3,
            4 => self.vmcb.save.cr4,
            _ => unreachable!(),
        }
    }

    fn set_cr(&mut self, cr_idx: usize, val: u64) {
        match cr_idx {
            // Mask off architecturally reserved bits and NW, as we don't want
            // write-through caches while in root mode (G11).
            0 => self.vmcb.save.cr0 = val
                & !super::super::CR0_RESERVED
                & !Cr0Flags::NOT_WRITE_THROUGH.bits(),
            3 => self.vmcb.save.cr3 = val,
            // Mask off architecturally reserved bits (G11).
            4 => self.vmcb.save.cr4 = val & !super::super::CR4_RESERVED,
            _ => unreachable!(),
        }
    }

    // AreaSwap-class MSRs. #VMEXIT does not write them back into the VMCB
    // save area (a direct hardware access would be lost on the next
    // VMRUN), so the MSRPM routes every access here and the handlers
    // read/write the VMCB save fields that VMRUN loads.
    fn rdmsr_virt(&self, msr: u32) -> Option<u64> {
        let save = &self.vmcb.save;
        match msr {
            0x174 => Some(save.sysenter_cs),
            0x175 => Some(save.sysenter_esp),
            0x176 => Some(save.sysenter_eip),
            0x277 => Some(save.g_pat),
            // SVME is the hypervisor's own EFER bit; the guest never sees it.
            0xc000_0080 => Some(save.efer & !EferFlags::SECURE_VIRTUAL_MACHINE_ENABLE.bits()),
            0xc000_0081 => Some(save.star),
            0xc000_0082 => Some(save.lstar),
            0xc000_0083 => Some(save.cstar),
            0xc000_0084 => Some(save.sfmask),
            0xc000_0100 => Some(save.fs.base),
            0xc000_0101 => Some(save.gs.base),
            0xc000_0102 => Some(save.kernel_gs_base),
            _ => None,
        }
    }

    fn wrmsr_virt(&mut self, msr: u32, val: u64) -> HvResult {
        let save = &mut self.vmcb.save;
        match msr {
            0x174 => save.sysenter_cs = val,
            0x175 => save.sysenter_esp = val,
            0x176 => save.sysenter_eip = val,
            0x277 => save.g_pat = val,
            0xc000_0080 => {
                // The guest may only flip software-writable bits; SVME (and
                // LME/LMA from the VMCB setup) must stay set or the next
                // VMRUN would fail its consistency checks. (SCE arbitration
                // for HU-Enclave hooks in here later.)
                const EFER_WRITABLE: u64 = 0x1 | 1 << 11 | 1 << 14; // SCE | NXE | FFXSR
                save.efer = (save.efer & !EFER_WRITABLE) | (val & EFER_WRITABLE);
            }
            0xc000_0081 => save.star = val,
            0xc000_0082 => save.lstar = val,
            0xc000_0083 => save.cstar = val,
            0xc000_0084 => save.sfmask = val,
            0xc000_0100 => save.fs.base = val,
            0xc000_0101 => save.gs.base = val,
            0xc000_0102 => save.kernel_gs_base = val,
            _ => return hv_result_err!(EINVAL, format!("no VMCB backing for MSR {:#x}", msr)),
        }
        Ok(())
    }
}

impl Debug for Vcpu {
    fn fmt(&self, f: &mut Formatter) -> Result {
        f.debug_struct("Vcpu")
            .field("guest_regs", &self.guest_regs)
            .field("rip", &self.instr_pointer())
            .field("rsp", &self.stack_pointer())
            .field("rflags", &RFlags::from_bits_retain(self.rflags()))
            .field("cr0", &Cr0Flags::from_bits_retain(self.cr(0)))
            .field("cr3", &self.cr(3))
            .field("cr4", &Cr4Flags::from_bits_retain(self.cr(4)))
            .field("cs", &self.vmcb.save.cs)
            .finish()
    }
}

#[unsafe(naked)]
unsafe extern "sysv64" fn svm_run() -> ! {
    core::arch::naked_asm!(
        "vmrun rax",
        save_regs_to_stack!(),
        "mov r14, rax",         // save host RAX to r14 for VMRUN
        "mov r15, rsp",         // save temporary RSP to r15
        "mov rsp, [rsp + {0}]", // set RSP to Vcpu::host_stack_top
        "call {1}",
        "lea rsp, [r15 + 8]",   // load temporary RSP and skip one place for RAX
        "push r14",             // push saved RAX to restore RAX later
        restore_regs_from_stack!(),
        "jmp {2}",
        const core::mem::size_of::<GuestRegisters>(),
        sym crate::arch::vmm::vmexit_handler,
        sym svm_run,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The permission map must encode the policy table: passthrough stays
    /// clear, everything else is intercepted; MSRs outside the map cannot
    /// be trapped at all and keep bare-metal semantics.
    #[test]
    fn test_msrpm_follows_policy() {
        let mut map = [0u8; 3 * 2048];
        apply_policy(&mut map);

        let intercepted = |msr: u32, is_write: bool| -> bool {
            permission_bit(msr, is_write).map_or(false, |(byte, bit)| map[byte] & (1 << bit) != 0)
        };
        // Passthrough: no interception bits.
        assert!(!intercepted(0x10, false) && !intercepted(0x10, true)); // TSC
        assert!(!intercepted(0x6e0, true)); // TSC_DEADLINE
        assert!(!intercepted(0x808, false)); // x2APIC ICR
        assert!(!intercepted(0xc001_0015, true)); // AMD HWCR
        // Non-passthrough actions: both directions intercepted.
        assert!(intercepted(0x174, false) && intercepted(0x174, true)); // SYSENTER
        assert!(intercepted(0xc000_0080, false)); // EFER
        assert!(intercepted(0xc000_0102, false)); // KERNEL_GS_BASE
        assert!(intercepted(0x480, false)); // VMX capability zone: read hidden
        // Read-only platform MSRs: read passes, write denied.
        assert!(!intercepted(0xfe, false) && intercepted(0xfe, true)); // MTRRCAP
        // MSRs outside the three ranges cannot be trapped at all.
        assert!(permission_bit(0x2000, false).is_none());
        assert!(permission_bit(0x4000_0000, false).is_none());
        assert!(permission_bit(0xc002_0000, false).is_none());
    }
}
