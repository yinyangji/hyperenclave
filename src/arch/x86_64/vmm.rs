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

#[cfg(feature = "intel")]
#[path = "intel/mod.rs"]
mod vendor;

#[cfg(feature = "amd")]
#[path = "amd/mod.rs"]
mod vendor;

use x86_64::registers::control::{Cr0Flags, Cr4Flags};

use super::GuestRegisters;
use crate::{error::HvResult, percpu::PerCpu};

pub use vendor::{
    check_hypervisor_feature, EnclaveNestedPageTableUnlocked, IoPTEntry, IoPageTable, Iommu,
    NPTEntry, NestedPageTable, Vcpu,
};

#[cfg(feature = "amd")]
pub use vendor::{EncHW, HmacSWEncHW};

pub trait VcpuAccessGuestState {
    // Architecture independent methods:
    fn regs(&self) -> &GuestRegisters;
    fn regs_mut(&mut self) -> &mut GuestRegisters;
    fn instr_pointer(&self) -> u64;
    fn stack_pointer(&self) -> u64;
    fn frame_pointer(&self) -> u64 {
        self.regs().rbp
    }
    fn set_stack_pointer(&mut self, sp: u64);
    fn set_return_val(&mut self, ret_val: usize) {
        self.regs_mut().rax = ret_val as _
    }

    // Methods only available for x86 cpus:
    fn rflags(&self) -> u64;
    fn fs_base(&self) -> u64;
    fn gs_base(&self) -> u64;
    fn efer(&self) -> u64;
    fn cr(&self, cr_idx: usize) -> u64;
    fn set_cr(&mut self, cr_idx: usize, val: u64);
    fn xcr0(&self) -> u64 {
        unsafe { core::arch::x86_64::_xgetbv(0) }
    }
    fn set_xcr0(&mut self, val: u64) {
        unsafe { core::arch::x86_64::_xsetbv(0, val) };
    }

    // MSR virtualization: serve an AreaSwap-class MSR read from the
    // hardware-managed guest structure (VMCS guest field, MSR load/store
    // area, or VMCB save area). Returns None if this MSR has no virtualized
    // backing (programming error; callers treat it as a hard error).
    fn rdmsr_virt(&self, msr: u32) -> Option<u64>;
    // MSR virtualization: store an AreaSwap-class MSR write into the
    // hardware-managed guest structure so that the next VM entry loads it.
    fn wrmsr_virt(&mut self, msr: u32, val: u64) -> HvResult;
}

const VM_EXIT_LEN_CPUID: u8 = 2;
const VM_EXIT_LEN_HYPERCALL: u8 = 3;

const HOST_CR4: Cr4Flags = Cr4Flags::from_bits_truncate(
    Cr4Flags::PHYSICAL_ADDRESS_EXTENSION.bits() | Cr4Flags::OSXSAVE.bits(),
);

/// Architecturally reserved CR0 bits (must be zero on both vendors).
///
/// `Cr0Flags` covers every architecturally defined CR0 bit, so everything
/// outside it is reserved (Intel SDM Vol.3 §2.5, AMD APM Vol.2).
const CR0_RESERVED: u64 = !Cr0Flags::all().bits();
/// Architecturally reserved CR4 bits (must be zero on both vendors).
///
/// `Cr4Flags` covers every architecturally defined CR4 bit (bits 0-14 and
/// 16-24; bits 15 and 25-63 are reserved), so everything outside it is
/// reserved (Intel SDM Vol.3 §2.5, AMD APM Vol.2).
const CR4_RESERVED: u64 = !Cr4Flags::all().bits();

pub(super) struct VmExit<'a> {
    pub cpu_data: &'a mut PerCpu,
}

impl VmExit<'_> {
    pub fn new() -> Self {
        Self {
            cpu_data: PerCpu::from_local_base_mut(),
        }
    }

    pub fn handle_msr_read(&mut self) -> HvResult {
        super::msr::handle_rdmsr(self.cpu_data)
    }

    pub fn handle_msr_write(&mut self) -> HvResult {
        super::msr::handle_wrmsr(self.cpu_data)
    }

    pub fn handle_pio(&mut self, port: u16, is_read: bool, instr_len: u8) -> HvResult {
        super::pio::handle_pio(self.cpu_data, port, is_read, instr_len)
    }

    pub fn handle_cpuid(&mut self) -> HvResult {
        let (function, subfunction, guest_osxsave) = {
            let regs = self.cpu_data.vcpu.regs();
            let guest_osxsave =
                Cr4Flags::from_bits_truncate(self.cpu_data.vcpu.cr(4)).contains(Cr4Flags::OSXSAVE);
            (regs.rax as u32, regs.rcx as u32, guest_osxsave)
        };
        let result = self
            .cpu_data
            .cpuid_policy
            .emulate(function, subfunction, guest_osxsave);
        let regs = self.cpu_data.vcpu.regs_mut();
        regs.rax = result.eax as _;
        regs.rbx = result.ebx as _;
        regs.rcx = result.ecx as _;
        regs.rdx = result.edx as _;
        self.cpu_data.vcpu.advance_rip(VM_EXIT_LEN_CPUID)
    }

    pub fn handle_hypercall(&mut self) -> HvResult {
        use crate::hypercall::HyperCall;
        self.cpu_data.vcpu.advance_rip(VM_EXIT_LEN_HYPERCALL)?;
        let guest_regs = self.cpu_data.vcpu.regs();
        let (code, arg0, arg1) = (guest_regs.rax, guest_regs.rdi, guest_regs.rsi);
        match HyperCall::new(&mut self.cpu_data).hypercall(code as _, arg0, arg1) {
            None => (),
            Some(exception_info) => {
                self.cpu_data.vcpu.rollback_rip(VM_EXIT_LEN_HYPERCALL)?;
                self.inject_exception(exception_info)?;
            }
        };
        Ok(())
    }

    #[allow(dead_code)]
    fn test_read_guest_memory(&self, gvaddr: usize, size: usize) -> HvResult {
        use crate::cell;
        use crate::memory::{addr::phys_to_virt, GenericPageTableImmut};

        let pt = self.cpu_data.vcpu.guest_page_table();
        let (gpaddr, _, _) = pt.query(gvaddr)?;
        let (hpaddr, _, _) = cell::ROOT_CELL.gpm.page_table().query(gpaddr)?;
        println!(
            "GVA({:#x?}) -> GPA({:#x?}) -> HPA({:#x?}):",
            gvaddr, gpaddr, hpaddr
        );
        let buf = unsafe { core::slice::from_raw_parts(phys_to_virt(gpaddr) as *const u8, size) };
        println!("{:02X?}", buf);
        Ok(())
    }
}

pub(super) fn vmexit_handler() {
    let mut vmexit = VmExit::new();
    let res = vmexit.handle_exit();
    if let Err(err) = res {
        error!(
            "Failed to handle VM exit, inject fault to guest...\n{:?}",
            err
        );
        vmexit.cpu_data.fault().unwrap();
    }
}
